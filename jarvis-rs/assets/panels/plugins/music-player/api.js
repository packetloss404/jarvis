// Music API — communicates with Rust via IPC, streams via jarvis:// protocol.

const MusicAPI = {
  _allTracks: [],
  _musicDir: '',

  // Encode a file path for use in jarvis:// URLs (base64url).
  _encodePath(path) {
    // Handle Unicode paths: encode to UTF-8 bytes first
    const utf8 = unescape(encodeURIComponent(path));
    return btoa(utf8).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
  },

  // Initialize: load cached library from Rust.
  async init() {
    const resp = await window.jarvis.ipc.request('music_init', {});
    this._allTracks = resp.tracks || [];
    this._musicDir = resp.music_dir || '';
    return resp;
  },

  // Scan (or rescan) the music directory.
  async scan(path) {
    const resp = await window.jarvis.ipc.request('music_scan', { path: path || undefined });
    this._allTracks = resp.tracks || [];
    this._musicDir = resp.music_dir || '';
    return resp;
  },

  // Change the music directory.
  async setDir(path) {
    return window.jarvis.ipc.request('music_set_dir', { path });
  },

  // Get streaming URL for a track.
  streamUrl(track) {
    return 'jarvis://localhost/music/stream/' + this._encodePath(track.path);
  },

  // Get cover art URL for a track (or null if no cover).
  artUrl(track) {
    if (!track.has_cover) return null;
    return 'jarvis://localhost/music/art/' + this._encodePath(track.path);
  },

  // --- Client-side library queries (no IPC needed) ---

  getArtists() {
    const map = {};
    for (const t of this._allTracks) {
      const name = t.artist || 'Unknown Artist';
      if (!map[name]) map[name] = { name, track_count: 0 };
      map[name].track_count++;
    }
    return Object.values(map).sort((a, b) => a.name.localeCompare(b.name));
  },

  getAlbums(artist) {
    const map = {};
    for (const t of this._allTracks) {
      if (artist && (t.artist || 'Unknown Artist') !== artist) continue;
      const name = t.album || 'Unknown Album';
      if (!map[name]) {
        map[name] = {
          name,
          artist: t.album_artist || t.artist || '',
          year: t.year || null,
          has_cover: false,
          cover_track: null,
        };
      }
      if (t.has_cover && !map[name].has_cover) {
        map[name].has_cover = true;
        map[name].cover_track = t;
      }
    }
    return Object.values(map).sort((a, b) => a.name.localeCompare(b.name));
  },

  getAlbumTracks(album) {
    return this._allTracks
      .filter(t => (t.album || 'Unknown Album') === album)
      .sort((a, b) => {
        const d = (a.disc_number || 1) - (b.disc_number || 1);
        if (d !== 0) return d;
        return (a.track_number || 0) - (b.track_number || 0);
      });
  },

  search(query) {
    const q = query.toLowerCase();
    return this._allTracks.filter(t =>
      (t.title || '').toLowerCase().includes(q) ||
      (t.artist || '').toLowerCase().includes(q) ||
      (t.album || '').toLowerCase().includes(q)
    );
  },

  get tracks() { return this._allTracks; },
  get musicDir() { return this._musicDir; },
};
