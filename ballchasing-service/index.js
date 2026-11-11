const express = require('express');
const cors = require('cors');
const fetch = (...args) => import('node-fetch').then(({default: fetch}) => fetch(...args));
const mongoose = require('mongoose');
const jwt = require('jsonwebtoken');
const bcrypt = require('bcryptjs');

const app = express();
app.use(cors());
app.use(express.json());

const PORT = process.env.PORT || 3001;
const MONGODB_URI = process.env.MONGODB_URI;
if (!MONGODB_URI) { console.error('FATAL ERROR: MONGODB_URI is missing.'); process.exit(1); }
mongoose.connect(MONGODB_URI).then(() => console.log('Connected to MongoDB')).catch(err => console.error('MongoDB error:', err));

// --- SCHEMAS ---
const settingsSchema = new mongoose.Schema({
  _id: { type: String, default: 'global_settings' },
  bannerText: { type: String, default: '' },
  bannerExpiry: { type: Date, default: null },
  seasonEnd: { type: Date, default: null },
  maintenanceMode: { type: Boolean, default: false },
  disabledFeatures: { type: [String], default: [] },
  popupText: { type: String, default: '' },
  popupExpiry: { type: Date, default: null },
  apiUsageCount: { type: Number, default: 0 }
});
const Settings = mongoose.model('Settings', settingsSchema);

const newsSchema = new mongoose.Schema({
  title: { type: String, required: true },
  content: { type: String, required: true },
  date: { type: Date, default: Date.now }
});
const News = mongoose.model('News', newsSchema);

const logSchema = new mongoose.Schema({
  action: { type: String, required: true },
  timestamp: { type: Date, default: Date.now }
});
const Log = mongoose.model('Log', logSchema);

const favoriteSchema = new mongoose.Schema({
  playerName: { type: String, required: true },
  platform: { type: String, required: true },
  count: { type: Number, default: 0 }
});
const Favorite = mongoose.model('Favorite', favoriteSchema);

// --- API USAGE TRACKER ---
let apiCallsSinceLastSync = 0;
app.use((req, res, next) => {
  if (req.path.startsWith('/api/') && !req.path.includes('/admin/')) apiCallsSinceLastSync++;
  next();
});
setInterval(async () => {
  if (apiCallsSinceLastSync > 0) {
    try {
      await Settings.findByIdAndUpdate('global_settings', { $inc: { apiUsageCount: apiCallsSinceLastSync } }, { upsert: true });
      apiCallsSinceLastSync = 0;
    } catch (e) {}
  }
}, 60000);

const logAction = async (action) => { try { await Log.create({ action }); } catch (e) {} };

// --- AUTH ---
const verifyToken = (req, res, next) => {
  const authHeader = req.headers['authorization'];
  const token = authHeader && authHeader.split(' ')[1];
  if (!token) return res.status(401).json({ error: 'Access denied' });
  try {
    jwt.verify(token, process.env.JWT_SECRET || 'fallback-super-secret-key');
    next();
  } catch (error) { res.status(401).json({ error: 'Invalid token' }); }
};

app.post('/api/auth/login', async (req, res) => {
  const { password } = req.body;
  if (!password || password !== process.env.ADMIN_PASSWORD) return res.status(401).json({ error: 'Invalid password' });
  const token = jwt.sign({ role: 'admin' }, process.env.JWT_SECRET || 'fallback-super-secret-key', { expiresIn: '12h' });
  res.json({ token });
});

// --- BALLCHASING API ---
const cache = new Map();
const CACHE_TTL = 5 * 60 * 1000; 
app.get('/api/player/:platform/:id/replays', async (req, res) => {
  const { platform, id } = req.params;
  const playerId = `${platform}:${id}`;
  const cacheKey = `replays:${playerId}`;
  const cached = cache.get(cacheKey);
  if (cached && Date.now() - cached.timestamp < CACHE_TTL) return res.json(cached.data);

  try {
    const response = await fetch(`https://ballchasing.com/api/replays?player-id=${playerId}&count=20`, {
      headers: { 'Authorization': process.env.BALLCHASING_API_KEY, 'Accept': 'application/json' }
    });
    if (!response.ok) throw new Error(`Ballchasing API error: ${response.status}`);
    const data = await response.json();
    const normalized = (data.list || []).map(r => ({
      id: r.id, date: r.date, playlist: r.playlist_id, duration: r.duration, map: r.map_name, teams: r.teams,
      players: r.players.map(p => ({
        name: p.name, platform: p.platform,
        stats: { score: p.stats?.score ?? 0, goals: p.stats?.goals ?? 0, assists: p.stats?.assists ?? 0, saves: p.stats?.saves ?? 0, shots: p.stats?.shots ?? 0, mvps: p.stats?.mvps ?? 0, possession: p.stats?.possession ?? 0, demos: p.stats?.demo?.inflicted ?? 0, boost_bpm: p.stats?.boost?.bpm ?? 0 }
      })),
    }));
    cache.set(cacheKey, { data: normalized, timestamp: Date.now() });
    res.json(normalized);
  } catch (err) { res.status(500).json({ error: 'Failed to fetch replay data' }); }
});

app.get('/api/leaderboard', async (req, res) => {
  res.json([
    { name: 'Mawkzy', mmr: 1950, wins: 150, losses: 50 },
    { name: 'Zen', mmr: 2100, wins: 200, losses: 40 },
    { name: 'Rise', mmr: 1800, wins: 120, losses: 60 }
  ]);
});

// --- STEAM API ---
let steamCache = { count: null, timestamp: 0 };
app.get('/api/steam-count', async (req, res) => {
  try {
    if (Date.now() - steamCache.timestamp < 300000) return res.json({ count: steamCache.count });
    const response = await fetch('https://api.steampowered.com/ISteamUserStats/GetNumberOfCurrentPlayers/v1/?appid=252950');
    const data = await response.json();
    steamCache = { count: data.response?.player_count || null, timestamp: Date.now() };
    res.json({ count: steamCache.count });
  } catch (error) { res.status(500).json({ count: null }); }
});

// --- FAVORITES ---
app.post('/api/favorites/toggle', async (req, res) => {
  const { name, platform, action } = req.body;
  try {
    if (action === 'add') await Favorite.findOneAndUpdate({ playerName: name, platform }, { $inc: { count: 1 } }, { upsert: true });
    else await Favorite.findOneAndUpdate({ playerName: name, platform }, { $inc: { count: -1 } });
    res.json({ success: true });
  } catch (e) { res.status(500).json({ error: 'Failed' }); }
});

// --- SETTINGS & PUBLIC DATA ---
app.get('/api/settings', async (req, res) => {
  try {
    let settings = await Settings.findById('global_settings');
    if (!settings) settings = await Settings.create({ _id: 'global_settings' });
    res.json(settings);
  } catch (e) { res.status(500).json({ error: 'Failed' }); }
});

app.get('/api/news', async (req, res) => {
  try { const news = await News.find().sort({ date: -1 }).limit(3); res.json(news); } catch (e) { res.status(500).json({ error: 'Failed' }); }
});

// --- ADMIN DASHBOARD ROUTES (Protected) ---
app.post('/api/settings', verifyToken, async (req, res) => {
  try {
    const newSettings = await Settings.findByIdAndUpdate('global_settings', req.body, { new: true, upsert: true });
    await logAction('Updated global settings');
    res.json({ success: true, settings: newSettings });
  } catch (e) { res.status(500).json({ error: 'Failed' }); }
});

app.get('/api/admin/backup', verifyToken, async (req, res) => {
  try {
    const settings = await Settings.findById('global_settings');
    await logAction('Downloaded database backup');
    res.setHeader('Content-Disposition', 'attachment; filename="rl_settings_backup.json"');
    res.setHeader('Content-Type', 'application/json');
    res.send(JSON.stringify(settings, null, 2));
  } catch (e) { res.status(500).json({ error: 'Failed' }); }
});

app.get('/api/admin/logs', verifyToken, async (req, res) => {
  try { const logs = await Log.find().sort({ timestamp: -1 }).limit(20); res.json(logs); } catch (e) { res.status(500).json({ error: 'Failed' }); }
});

app.get('/api/admin/favorites', verifyToken, async (req, res) => {
  try { const favs = await Favorite.find().sort({ count: -1 }).limit(10); res.json(favs); } catch (e) { res.status(500).json({ error: 'Failed' }); }
});

app.post('/api/admin/news', verifyToken, async (req, res) => {
  try {
    const post = await News.create(req.body);
    await logAction(`Created news post: ${req.body.title}`);
    res.json({ success: true, post });
  } catch (e) { res.status(500).json({ error: 'Failed' }); }
});

// NEW: Edit News Route
app.put('/api/admin/news/:id', verifyToken, async (req, res) => {
  try {
    const post = await News.findByIdAndUpdate(req.params.id, req.body, { new: true });
    await logAction(`Edited news post: ${post.title}`);
    res.json({ success: true, post });
  } catch (e) { res.status(500).json({ error: 'Failed' }); }
});

app.delete('/api/admin/news/:id', verifyToken, async (req, res) => {
  try {
    await News.findByIdAndDelete(req.params.id);
    await logAction(`Deleted a news post`);
    res.json({ success: true });
  } catch (e) { res.status(500).json({ error: 'Failed' }); }
});

app.get('/api/health', (req, res) => res.json({ status: 'ok' }));
app.listen(PORT, '0.0.0.0', () => console.log(`Service running on :${PORT}`));
