const express = require('express');
const cors = require('cors');
const fetch =
  globalThis.fetch ||
  ((...args) => import('node-fetch').then(({ default: f }) => f(...args)));
const mongoose = require('mongoose');
const jwt = require('jsonwebtoken');
const bcrypt = require('bcryptjs');

const app = express();
app.use(cors());
app.options('*', cors());
app.use(express.json());

const PORT = process.env.PORT || 3001;
const MONGODB_URI = (process.env.MONGODB_URI || '').trim();
const PSYNET_SERVICE_URL = (
  process.env.PSYNET_SERVICE_URL || 'https://rltracker-g6d7.onrender.com'
)
  .trim()
  .replace(/\/+$/, '');
const INTERNAL_API_TOKEN = (
  process.env.INTERNAL_API_TOKEN || 'default_token'
).trim();

if (!MONGODB_URI) {
  console.error('FATAL ERROR: MONGODB_URI is missing.');
  process.exit(1);
}
mongoose
  .connect(MONGODB_URI)
  .then(() => console.log('Connected to MongoDB'))
  .catch((err) => console.error('MongoDB error:', err));

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
  popupId: { type: String, default: '' },
  apiUsageCount: { type: Number, default: 0 },
});
const Settings = mongoose.model('Settings', settingsSchema);

const newsSchema = new mongoose.Schema({
  title: { type: String, required: true },
  content: { type: String, required: true },
  date: { type: Date, default: Date.now },
});
const News = mongoose.model('News', newsSchema);

const logSchema = new mongoose.Schema({
  action: { type: String, required: true },
  timestamp: { type: Date, default: Date.now },
});
const Log = mongoose.model('Log', logSchema);

const favoriteSchema = new mongoose.Schema({
  playerName: { type: String, required: true },
  platform: { type: String, required: true },
  count: { type: Number, default: 0 },
});
const Favorite = mongoose.model('Favorite', favoriteSchema);

const mmrSnapshotSchema = new mongoose.Schema({
  platform: { type: String, required: true, index: true },
  playerKey: { type: String, required: true, index: true },
  playlist: { type: String, required: true, index: true },
  mmr: { type: Number, required: true },
  tier: { type: Number, default: 0 },
  timestamp: { type: Date, default: Date.now, index: true },
});
const MmrSnapshot = mongoose.model('MmrSnapshot', mmrSnapshotSchema);

// --- API USAGE TRACKER ---
let apiCallsSinceLastSync = 0;
app.use((req, res, next) => {
  if (req.path.startsWith('/api/') && !req.path.includes('/admin/')) {
    apiCallsSinceLastSync++;
  }
  next();
});
setInterval(async () => {
  if (apiCallsSinceLastSync > 0) {
    try {
      await Settings.findByIdAndUpdate(
        'global_settings',
        { $inc: { apiUsageCount: apiCallsSinceLastSync } },
        { upsert: true }
      );
      apiCallsSinceLastSync = 0;
    } catch (e) {}
  }
}, 60000);

const logAction = async (action) => {
  try {
    await Log.create({ action });
  } catch (e) {}
};

// --- AUTH ---
const verifyToken = (req, res, next) => {
  const authHeader = req.headers['authorization'];
  const token = authHeader && authHeader.split(' ')[1];
  if (!token) return res.status(401).json({ error: 'Access denied' });
  try {
    jwt.verify(token, process.env.JWT_SECRET || 'fallback-super-secret-key');
    next();
  } catch (error) {
    res.status(401).json({ error: 'Invalid token' });
  }
};

app.post('/api/auth/login', async (req, res) => {
  const { password } = req.body;
  if (!password || password !== process.env.ADMIN_PASSWORD) {
    return res.status(401).json({ error: 'Invalid password' });
  }
  const token = jwt.sign(
    { role: 'admin' },
    process.env.JWT_SECRET || 'fallback-super-secret-key',
    { expiresIn: '12h' }
  );
  res.json({ token });
});

// --- PSYNET RANKS PROXY + MMR HISTORY RECORDER ---
const cache = new Map();
const epicIdCache = new Map();
const CACHE_TTL = 2 * 60 * 1000;

async function fetchWithTimeout(url, options = {}, timeoutMs = 25000) {
  const controller = new AbortController();
  const id = setTimeout(() => controller.abort(), timeoutMs);
  try {
    return await fetch(url, { ...options, signal: controller.signal });
  } finally {
    clearTimeout(id);
  }
}

async function resolveSteamIdIfNeeded(id) {
  const trimmed = String(id).trim();
  if (/^\d{17}$/.test(trimmed)) return trimmed;
  if (!process.env.STEAM_API_KEY) return trimmed;
  try {
    const r = await fetchWithTimeout(
      `https://api.steampowered.com/ISteamUser/ResolveVanityURL/v1/?key=${process.env.STEAM_API_KEY}&vanityurl=${encodeURIComponent(trimmed)}`,
      {},
      10000
    );
    const data = await r.json();
    if (data?.response?.success === 1 && data.response.steamid) {
      return data.response.steamid;
    }
  } catch (e) {}
  return trimmed;
}

app.get('/api/ranks/:platform/:id', async (req, res) => {
  const platform = req.params.platform.toLowerCase();
  const rawId = req.params.id.trim();
  const resolvedId =
    platform === 'steam' ? await resolveSteamIdIfNeeded(rawId) : rawId;
  const playerKey = resolvedId.toLowerCase();
  const cacheKey = `ranks:${platform}:${playerKey}`;

  const cached = cache.get(cacheKey);
  if (cached && Date.now() - cached.timestamp < CACHE_TTL) {
    return res.json(cached.data);
  }

  try {
    const upstream = await fetchWithTimeout(
      `${PSYNET_SERVICE_URL}/v1/ranks/${encodeURIComponent(platform)}/${encodeURIComponent(resolvedId)}`,
      {
        headers: {
          Authorization: `Bearer ${INTERNAL_API_TOKEN}`,
          Accept: 'application/json',
        },
      },
      28000
    );

    const text = await upstream.text();
    if (!upstream.ok) {
      return res.status(upstream.status).json({
        error: text || `PsyNet service returned HTTP ${upstream.status}`,
      });
    }

    const ranks = JSON.parse(text);

    if (platform === 'epic' && ranks[0]?.account_id) {
      epicIdCache.set(playerKey, ranks[0].account_id);
      epicIdCache.set(ranks[0].account_id.toLowerCase(), ranks[0].account_id);
    }

    const storageKey =
      platform === 'epic' && ranks[0]?.account_id
        ? ranks[0].account_id.toLowerCase()
        : playerKey;

    for (const r of ranks) {
      const pl = String(r.playlist);
      const currentMmr = Number(r.mmr) || 0;
      if (currentMmr <= 0) continue;

      try {
        const latest = await MmrSnapshot.findOne({
          platform,
          playerKey: storageKey,
          playlist: pl,
        }).sort({ timestamp: -1 });

        const shouldInsert =
          !latest ||
          latest.mmr !== currentMmr ||
          Date.now() - new Date(latest.timestamp).getTime() > 30 * 60 * 1000;

        if (shouldInsert) {
          await MmrSnapshot.create({
            platform,
            playerKey: storageKey,
            playlist: pl,
            mmr: currentMmr,
            tier: Number(r.tier) || 0,
          });
        }

        const peakDoc = await MmrSnapshot.findOne({
          platform,
          playerKey: storageKey,
          playlist: pl,
        }).sort({ mmr: -1 });

        if (peakDoc && peakDoc.mmr > currentMmr) {
          r.peak_mmr = peakDoc.mmr;
          r.peak_tier = peakDoc.tier || r.tier;
        }
      } catch (e) {}
    }

    cache.set(cacheKey, { data: ranks, timestamp: Date.now() });
    res.json(ranks);
  } catch (err) {
    console.error('Ranks proxy error:', err);
    res.status(502).json({
      error: `Failed to reach PsyNet service: ${err.message}`,
    });
  }
});

app.get('/api/player/:platform/:id/mmr-history', async (req, res) => {
  const platform = req.params.platform.toLowerCase();
  const rawId = req.params.id.trim();
  const resolvedId =
    platform === 'steam' ? await resolveSteamIdIfNeeded(rawId) : rawId;
  const cachedEpic =
    platform === 'epic' ? epicIdCache.get(resolvedId.toLowerCase()) : null;
  const playerKey = (cachedEpic || resolvedId).toLowerCase();
  const playlist = String(req.query.playlist || '11');
  const days = Math.min(365, Math.max(1, Number(req.query.days) || 30));
  const since = new Date(Date.now() - days * 24 * 60 * 60 * 1000);

  try {
    const snapshots = await MmrSnapshot.find({
      platform,
      playerKey,
      playlist,
      timestamp: { $gte: since },
    })
      .sort({ timestamp: 1 })
      .limit(200)
      .lean();

    res.json(snapshots.map((s) => ({ timestamp: s.timestamp, mmr: s.mmr })));
  } catch (err) {
    res.status(500).json({ error: 'Failed to load MMR history' });
  }
});

// --- BALLCHASING API ---
const REPLAY_CACHE_TTL = 5 * 60 * 1000;
app.get('/api/player/:platform/:id/replays', async (req, res) => {
  const platform = req.params.platform.toLowerCase();
  const rawId = req.params.id.trim();
  const resolvedEpicId =
    platform === 'epic' ? epicIdCache.get(rawId.toLowerCase()) : null;
  const lookupId = resolvedEpicId || rawId;
  const cacheKey = `replays:${platform}:${lookupId.toLowerCase()}`;

  const cached = cache.get(cacheKey);
  if (cached && Date.now() - cached.timestamp < REPLAY_CACHE_TTL) {
    return res.json(cached.data);
  }

  try {
    const headers = {
      Authorization: process.env.BALLCHASING_API_KEY,
      Accept: 'application/json',
    };

    let data = { list: [] };

    // 1. Try player-id with resolved ID (32-char Epic ID or SteamID64)
    const r1 = await fetchWithTimeout(
      `https://ballchasing.com/api/replays?player-id=${encodeURIComponent(`${platform}:${lookupId}`)}&count=20`,
      { headers },
      15000
    );
    if (r1.ok) data = await r1.json();

    // 2. Fallback to player-name search if player-id returned empty
    if (!data.list || data.list.length === 0) {
      const r2 = await fetchWithTimeout(
        `https://ballchasing.com/api/replays?player-name=${encodeURIComponent(rawId)}&count=20`,
        { headers },
        15000
      );
      if (r2.ok) data = await r2.json();
    }

    const normalized = (data.list || []).map((r) => ({
      id: r.id,
      date: r.date,
      playlist: r.playlist_id,
      duration: r.duration,
      map: r.map_name,
      teams: r.teams,
      season: r.season,
      players: (r.players || []).map((p) => ({
        name: p.name,
        platform: p.platform,
        stats: {
          score: p.stats?.score ?? 0,
          goals: p.stats?.goals ?? 0,
          assists: p.stats?.assists ?? 0,
          saves: p.stats?.saves ?? 0,
          shots: p.stats?.shots ?? 0,
          mvps: p.stats?.mvps ?? 0,
          possession: p.stats?.possession ?? 0,
          demos: p.stats?.demo?.inflicted ?? 0,
          boost_bpm: p.stats?.boost?.bpm ?? 0,
        },
      })),
    }));

    cache.set(cacheKey, { data: normalized, timestamp: Date.now() });
    res.json(normalized);
  } catch (err) {
    console.error('Ballchasing replays error:', err);
    res.status(500).json({ error: `Failed to fetch replay data: ${err.message}` });
  }
});

app.get('/api/leaderboard', async (req, res) => {
  const playlist = String(req.query.playlist || '11');
  const limit = Math.min(100, Math.max(1, Number(req.query.limit) || 25));

  const presets = {
    10: [
      { name: 'Mawkzy', mmr: 1840, wins: 192, losses: 54 },
      { name: 'Rw9', mmr: 1815, wins: 175, losses: 49 },
      { name: 'ApparentlyJack', mmr: 1780, wins: 164, losses: 58 },
    ],
    11: [
      { name: 'zenrl', mmr: 2658, wins: 310, losses: 62 },
      { name: 'Vatira', mmr: 2590, wins: 288, losses: 71 },
      { name: 'dralii', mmr: 2564, wins: 274, losses: 69 },
    ],
    13: [
      { name: 'GGM1 JUSTIN.', mmr: 2070, wins: 215, losses: 64 },
      { name: 'trk511', mmr: 2045, wins: 198, losses: 59 },
      { name: 'BeastMode', mmr: 2022, wins: 190, losses: 61 },
    ],
  };

  const rows = presets[playlist] || presets['11'];
  res.json(rows.slice(0, limit));
});

// --- STEAM API ---
let steamCache = { count: null, timestamp: 0 };
app.get('/api/steam-count', async (req, res) => {
  try {
    if (Date.now() - steamCache.timestamp < 300000) {
      return res.json({ count: steamCache.count });
    }
    const response = await fetchWithTimeout(
      'https://api.steampowered.com/ISteamUserStats/GetNumberOfCurrentPlayers/v1/?appid=252950',
      {},
      10000
    );
    const data = await response.json();
    steamCache = {
      count: data.response?.player_count || null,
      timestamp: Date.now(),
    };
    res.json({ count: steamCache.count });
  } catch (error) {
    res.status(500).json({ count: null });
  }
});

// --- FAVORITES ---
app.post('/api/favorites/toggle', async (req, res) => {
  const { name, platform, action } = req.body;
  try {
    if (action === 'add') {
      await Favorite.findOneAndUpdate(
        { playerName: name, platform },
        { $inc: { count: 1 } },
        { upsert: true }
      );
    } else {
      await Favorite.findOneAndUpdate(
        { playerName: name, platform },
        { $inc: { count: -1 } }
      );
    }
    res.json({ success: true });
  } catch (e) {
    res.status(500).json({ error: 'Failed' });
  }
});

// --- SETTINGS & PUBLIC DATA ---
app.get('/api/settings', async (req, res) => {
  try {
    let settings = await Settings.findById('global_settings');
    if (!settings) settings = await Settings.create({ _id: 'global_settings' });
    res.json(settings);
  } catch (e) {
    res.status(500).json({ error: 'Failed' });
  }
});

app.get('/api/news', async (req, res) => {
  try {
    const news = await News.find().sort({ date: -1 }).limit(3);
    res.json(news);
  } catch (e) {
    res.status(500).json({ error: 'Failed' });
  }
});

// --- ADMIN DASHBOARD ROUTES (Protected) ---
app.post('/api/settings', verifyToken, async (req, res) => {
  try {
    const newSettings = await Settings.findByIdAndUpdate(
      'global_settings',
      req.body,
      { new: true, upsert: true }
    );
    await logAction('Updated global settings');
    res.json({ success: true, settings: newSettings });
  } catch (e) {
    res.status(500).json({ error: 'Failed' });
  }
});

app.get('/api/admin/backup', verifyToken, async (req, res) => {
  try {
    const settings = await Settings.findById('global_settings');
    await logAction('Downloaded database backup');
    res.setHeader(
      'Content-Disposition',
      'attachment; filename="rl_settings_backup.json"'
    );
    res.setHeader('Content-Type', 'application/json');
    res.send(JSON.stringify(settings, null, 2));
  } catch (e) {
    res.status(500).json({ error: 'Failed' });
  }
});

app.get('/api/admin/logs', verifyToken, async (req, res) => {
  try {
    const logs = await Log.find().sort({ timestamp: -1 }).limit(20);
    res.json(logs);
  } catch (e) {
    res.status(500).json({ error: 'Failed' });
  }
});

app.get('/api/admin/favorites', verifyToken, async (req, res) => {
  try {
    const favs = await Favorite.find().sort({ count: -1 }).limit(10);
    res.json(favs);
  } catch (e) {
    res.status(500).json({ error: 'Failed' });
  }
});

app.post('/api/admin/news', verifyToken, async (req, res) => {
  try {
    const post = await News.create(req.body);
    await logAction(`Created news post: ${req.body.title}`);
    res.json({ success: true, post });
  } catch (e) {
    res.status(500).json({ error: 'Failed' });
  }
});

app.put('/api/admin/news/:id', verifyToken, async (req, res) => {
  try {
    const post = await News.findByIdAndUpdate(req.params.id, req.body, {
      new: true,
    });
    await logAction(`Edited news post: ${post.title}`);
    res.json({ success: true, post });
  } catch (e) {
    res.status(500).json({ error: 'Failed' });
  }
});

app.delete('/api/admin/news/:id', verifyToken, async (req, res) => {
  try {
    await News.findByIdAndDelete(req.params.id);
    await logAction(`Deleted a news post`);
    res.json({ success: true });
  } catch (e) {
    res.status(500).json({ error: 'Failed' });
  }
});

app.get('/api/health', (req, res) => res.json({ status: 'ok' }));
app.listen(PORT, '0.0.0.0', () => console.log(`Service running on :${PORT}`));
