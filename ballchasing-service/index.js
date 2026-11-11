const express = require('express');
const cors = require('cors');
const fetch = (...args) => import('node-fetch').then(({default: fetch}) => fetch(...args));
const fs = require('fs');
const path = require('path');

const app = express();
app.use(cors());
app.use(express.json());
const PORT = process.env.PORT || 3001;

const SETTINGS_FILE = path.join(__dirname, 'settings.json');

// Ensure settings file exists with default structure
if (!fs.existsSync(SETTINGS_FILE)) {
  fs.writeFileSync(SETTINGS_FILE, JSON.stringify({ bannerText: '', bannerExpiry: null, seasonEnd: null }));
}

const cache = new Map();
const CACHE_TTL = 5 * 60 * 1000; 

// --- BALLCHASING ROUTES ---
app.get('/api/player/:platform/:id/replays', async (req, res) => {
  const { platform, id } = req.params;
  const playerId = `${platform}:${id}`;
  const cacheKey = `replays:${playerId}`;

  const cached = cache.get(cacheKey);
  if (cached && Date.now() - cached.timestamp < CACHE_TTL) {
    return res.json(cached.data);
  }

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
        stats: { score: p.stats?.score ?? 0, goals: p.stats?.goals ?? 0, assists: p.stats?.assists ?? 0, saves: p.stats?.saves ?? 0, shots: p.stats?.shots ?? 0 }
      })),
    }));

    cache.set(cacheKey, { data: normalized, timestamp: Date.now() });
    res.json(normalized);
  } catch (err) {
    console.error('Ballchasing error:', err.message);
    res.status(500).json({ error: 'Failed to fetch replay data' });
  }
});

// --- STEAM API ROUTE ---
app.get('/api/steam-count', async (req, res) => {
  try {
    const response = await fetch('https://api.steampowered.com/ISteamUserStats/GetNumberOfCurrentPlayers/v1/?appid=252950');
    const data = await response.json();
    res.json({ count: data.response?.player_count || null });
  } catch (error) {
    console.error('Steam API error:', error.message);
    res.status(500).json({ count: null });
  }
});

// --- SETTINGS ROUTES ---
app.get('/api/settings', (req, res) => {
  try {
    const raw = fs.readFileSync(SETTINGS_FILE, 'utf8');
    const settings = JSON.parse(raw);
    // Merge with defaults to ensure seasonEnd exists if using an old file
    res.json({ bannerText: '', bannerExpiry: null, seasonEnd: null, ...settings });
  } catch (e) {
    res.json({ bannerText: '', bannerExpiry: null, seasonEnd: null });
  }
});

app.post('/api/settings', (req, res) => {
  const authHeader = req.headers.authorization;
  const adminPassword = process.env.ADMIN_PASSWORD || 'f8b3d9c1e4a72b5f6d8e9a0c1b2d3e4f5a6b7c8d9e0f1a2b3c4d5e6f7a8b9c0d';
  
  if (authHeader !== `Bearer ${adminPassword}`) {
    return res.status(401).json({ error: 'Unauthorized' });
  }

  const { bannerText, bannerExpiry, seasonEnd } = req.body;
  const newSettings = { bannerText, bannerExpiry, seasonEnd };
  fs.writeFileSync(SETTINGS_FILE, JSON.stringify(newSettings, null, 2));
  res.json({ success: true, settings: newSettings });
});

app.get('/api/health', (req, res) => res.json({ status: 'ok' }));

// CRITICAL: Bind to 0.0.0.0 for Render
app.listen(PORT, '0.0.0.0', () => console.log(`Service running on :${PORT}`));
