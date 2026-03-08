class MusicAPI {
  constructor() {
    this.serverUrl = '';
    this.token = '';
  }

  configure(serverUrl, token) {
    // Remove trailing slash
    this.serverUrl = serverUrl.replace(/\/+$/, '');
    this.token = token;
  }

  async _fetch(path, options = {}) {
    const url = `${this.serverUrl}${path}`;
    const headers = {
      'Authorization': `Bearer ${this.token}`,
      ...options.headers,
    };
    const resp = await fetch(url, { ...options, headers });
    if (!resp.ok) {
      throw new Error(`API error: ${resp.status} ${resp.statusText}`);
    }
    return resp.json();
  }

  streamUrl(trackId) {
    return `${this.serverUrl}/stream/${trackId}?token=${encodeURIComponent(this.token)}`;
  }

  artUrl(trackId) {
    return `${this.serverUrl}/art/${trackId}?token=${encodeURIComponent(this.token)}`;
  }

  async health() {
    const resp = await fetch(`${this.serverUrl}/health`);
    return resp.ok;
  }

  async getArtists() {
    return this._fetch('/library/artists');
  }

  async getAlbums(artist) {
    const params = artist ? `?artist=${encodeURIComponent(artist)}` : '';
    return this._fetch(`/library/albums${params}`);
  }

  async getAlbumTracks(album) {
    return this._fetch(`/library/albums/${encodeURIComponent(album)}/tracks`);
  }

  async getTracks(offset = 0, limit = 100) {
    return this._fetch(`/library/tracks?offset=${offset}&limit=${limit}`);
  }

  async search(query) {
    return this._fetch(`/library/search?q=${encodeURIComponent(query)}`);
  }

  async rescan() {
    return this._fetch('/library/rescan', { method: 'POST' });
  }
}

const api = new MusicAPI();
