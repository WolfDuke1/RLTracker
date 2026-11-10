const express = require('express');
const cors = require('cors');
const fetch = (...args) => import('node-fetch').then(({default: fetch}) => fetch(...args));

const app = express();
app.use(cors());
const PORT = process.env.PORT || 3001;

// Cache to avoid hammering the Ballchasing API
const cache = new Map();
const CACHE_TTL = 5 * 60 * 1000; // 5 minutes

app.get('/api/player/:platform/:id/replays', async (req, res) => {
  const { platform, id } = req.params;
  const playerId = `${platform}:${id}`;
  const cacheKey = `replays:${playerId}`;

  const cached = cache.get(cacheKey);
  if (cached && Date.now() - cached.timestamp < CACHE_TTL) {
    return res.json(cached.data);
  }

  try {
    // Fetch directly from ballchasing.com
    const response = await fetch(`https://ballchasing.com/api/replays?player-id=${playerId}&count=20`, {
      headers: {
        'Authorization': process.env.BALLCHASING_API_KEY,
        'Accept': 'application/json'
      }
    });

    if (!response.ok) throw new Error(`Ballchasing API error: ${response.status}`);
    
    const data = await response.json();
    
    // Normalize for the frontend
    const normalized = (data.list || []).map(r => ({
      id: r.id,
      date: r.date,
      playlist: r.playlist_id,
      duration: r.duration,
      map: r.map_name,
      teams: r.teams,
      players: r.players.map(p => ({
        name: p.name,
        platform: p.platform,
        stats: {
          score: p.stats?.score ?? 0,
          goals: p.stats?.goals ?? 0,
          assists: p.stats?.assists ?? 0,
          saves: p.stats?.saves ?? 0,
          shots: p.stats?.shots ?? 0,
        },
      })),
    }));

    cache.set(cacheKey, { data: normalized, timestamp: Date.now() });
    res.json(normalized);
  } catch (err) {
    console.error('Ballchasing error:', err.message);
    res.status(500).json({ error: 'Failed to fetch replay data' });
  }
});

app.get('/api/health', (req, res) => res.json({ status: 'ok' }));
app.listen(PORT, () => console.log(`Ballchasing service on :${PORT}`));
