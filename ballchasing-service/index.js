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

// --- DATABASE CONNECTION ---
const MONGODB_URI = process.env.MONGODB_URI;
if (!MONGODB_URI) {
  console.error('FATAL ERROR: MONGODB_URI is not defined.');
  process.exit(1);
}

mongoose.connect(MONGODB_URI)
  .then(() => console.log('Connected to MongoDB Atlas'))
  .catch(err => console.error('MongoDB connection error:', err));

// --- SCHEMAS ---
const settingsSchema = new mongoose.Schema({
  _id: { type: String, default: 'global_settings' },
  bannerText: { type: String, default: '' },
  bannerExpiry: { type: Date, default: null },
  seasonEnd: { type: Date, default: null }
});
const Settings = mongoose.model('Settings', settingsSchema);

const userSchema = new mongoose.Schema({
  username: { type: String, required: true, unique: true },
  password: { type: String, required: true } // Hashed
});
const User = mongoose.model('User', userSchema);

// --- STEAM API CACHE ---
let steamCache = { count: null, timestamp: 0 };
const STEAM_CACHE_TTL = 5 * 60 * 1000; // 5 minutes

app.get('/api/steam-count', async (req, res) => {
  try {
    if (Date.now() - steamCache.timestamp < STEAM_CACHE_TTL) {
      return res.json({ count: steamCache.count });
    }
    const response = await fetch('https://api.steampowered.com/ISteamUserStats/GetNumberOfCurrentPlayers/v1/?appid=252950');
    const data = await response.json();
    const count = data.response?.player_count || null;
    
    steamCache = { count, timestamp: Date.now() };
    res.json({ count });
  } catch (error) {
    console.error('Steam API error:', error.message);
    res.status(500).json({ count: null });
  }
});

// --- BALLCHASING API (with Advanced Stats) ---
const cache = new Map();
const CACHE_TTL = 5 * 60 * 1000; 

app.get('/api/player/:platform/:id/replays', async (req, res) => {
  const { platform, id } = req.params;
  const playerId = `${platform}:${id}`;
  const cacheKey = `replays:${playerId}`;

  const cached = cache.get(cacheKey);
  if (cached && Date.now() - cached.timestamp < CACHE_TTL) {
    return res.json(cached.data);
  }

  try {
    // Requesting full replay details to get advanced stats
    const response = await fetch(`https://ballchasing.com/api/replays?player-id=${playerId}&count=20`, {
      headers: { 'Authorization': process.env.BALLCHASING_API_KEY, 'Accept': 'application/json' }
    });
    if (!response.ok) throw new Error(`Ballchasing API error: ${response.status}`);
    const data = await response.json();
    
    const normalized = (data.list || []).map(r => ({
      id: r.id, date: r.date, playlist: r.playlist_id, duration: r.duration, map: r.map_name, teams: r.teams,
      players: r.players.map(p => ({
        name: p.name, platform: p.platform,
        stats: { 
          score: p.stats?.score ?? 0, goals: p.stats?.goals ?? 0, assists: p.stats?.assists ?? 0, 
          saves: p.stats?.saves ?? 0, shots: p.stats?.shots ?? 0, mvps: p.stats?.mvps ?? 0,
          // Advanced Stats
          possession: p.stats?.possession ?? 0,
          demos: p.stats?.demo?.inflicted ?? 0,
          boost_bpm: p.stats?.boost?.bpm ?? 0,
          boost_collected: p.stats?.boost?.amount_collected ?? 0
        }
      })),
    }));

    cache.set(cacheKey, { data: normalized, timestamp: Date.now() });
    res.json(normalized);
  } catch (err) {
    console.error('Ballchasing error:', err.message);
    res.status(500).json({ error: 'Failed to fetch replay data' });
  }
});

// --- AUTHENTICATION ROUTES ---
app.post('/api/auth/login', async (req, res) => {
  const { password } = req.body;
  const adminPassword = process.env.ADMIN_PASSWORD;

  if (!password || password !== adminPassword) {
    return res.status(401).json({ error: 'Invalid password' });
  }

  // Generate JWT
  const token = jwt.sign(
    { role: 'admin' }, 
    process.env.JWT_SECRET || 'fallback-super-secret-key', 
    { expiresIn: '12h' }
  );
  res.json({ token });
});

// Middleware to verify JWT
const verifyToken = (req, res, next) => {
  const authHeader = req.headers['authorization'];
  const token = authHeader && authHeader.split(' ')[1];
  if (!token) return res.status(401).json({ error: 'Access denied' });

  try {
    const verified = jwt.verify(token, process.env.JWT_SECRET || 'fallback-super-secret-key');
    req.user = verified;
    next();
  } catch (error) {
    res.status(401).json({ error: 'Invalid token' });
  }
};

// --- SETTINGS ROUTES (Protected) ---
app.get('/api/settings', async (req, res) => {
  try {
    let settings = await Settings.findById('global_settings');
    if (!settings) {
      settings = await Settings.create({ _id: 'global_settings' });
    }
    res.json(settings);
  } catch (e) {
    res.status(500).json({ error: 'Failed to load settings' });
  }
});

app.post('/api/settings', verifyToken, async (req, res) => {
  const { bannerText, bannerExpiry, seasonEnd } = req.body;
  try {
    const newSettings = await Settings.findByIdAndUpdate(
      'global_settings',
      { bannerText, bannerExpiry, seasonEnd },
      { new: true, upsert: true }
    );
    res.json({ success: true, settings: newSettings });
  } catch (e) {
    res.status(500).json({ error: 'Failed to save settings' });
  }
});

app.get('/api/health', (req, res) => res.json({ status: 'ok' }));

app.listen(PORT, '0.0.0.0', () => console.log(`Service running on :${PORT}`));
