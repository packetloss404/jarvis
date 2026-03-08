(function() {
  'use strict';

  // State
  let currentView = 'artists';
  let selectedArtist = null;
  let selectedAlbum = null;

  // DOM refs
  const $ = (id) => document.getElementById(id);
  const setupScreen = $('setup-screen');
  const mainUI = $('main-ui');
  const transport = $('transport');
  const contentHeader = $('content-header');
  const contentList = $('content-list');
  const sidebarList = $('sidebar-list');
  const searchInput = $('search-input');
  const nowTitle = $('now-playing-title');
  const nowArtist = $('now-playing-artist');
  const btnPlay = $('btn-play');
  const timeCurrent = $('time-current');
  const timeTotal = $('time-total');
  const progressFill = $('progress-fill');
  const progressSeek = $('progress-seek');
  const volumeSlider = $('volume-slider');

  // --- Auth / Connection ---

  async function init() {
    let serverUrl = localStorage.getItem('music-player:server-url');
    let token = localStorage.getItem('music-player:token');

    if (!serverUrl || !token) {
      try {
        const resp = await fetch('config.json');
        if (resp.ok) {
          const cfg = await resp.json();
          serverUrl = cfg.server;
          token = cfg.token;
          localStorage.setItem('music-player:server-url', serverUrl);
          localStorage.setItem('music-player:token', token);
        }
      } catch (e) {
        // config.json not available
      }
    }

    if (serverUrl && token) {
      api.configure(serverUrl, token);
      try {
        await api.health();
        showMainUI();
        return;
      } catch (e) {
        // Server not reachable, show setup
      }
    }

    showSetup();
  }

  const loadingScreen = $('loading-screen');

  function showSetup() {
    loadingScreen.classList.add('hidden');
    setupScreen.classList.remove('hidden');
    mainUI.classList.add('hidden');
    transport.classList.add('hidden');
  }

  function showMainUI() {
    loadingScreen.classList.add('hidden');
    setupScreen.classList.add('hidden');
    mainUI.classList.remove('hidden');
    transport.classList.remove('hidden');
    loadView('artists');
  }

  // Setup form
  $('setup-connect').addEventListener('click', async () => {
    const url = $('setup-url').value.trim();
    const token = $('setup-token').value.trim();
    const errEl = $('setup-error');

    if (!url || !token) {
      errEl.textContent = 'Both fields are required.';
      errEl.classList.remove('hidden');
      return;
    }

    api.configure(url, token);
    try {
      await api.health();
      localStorage.setItem('music-player:server-url', url);
      localStorage.setItem('music-player:token', token);
      errEl.classList.add('hidden');
      showMainUI();
    } catch (e) {
      errEl.textContent = 'Cannot connect to server. Check URL and try again.';
      errEl.classList.remove('hidden');
    }
  });

  // --- Navigation ---

  document.querySelectorAll('.nav-item').forEach(btn => {
    btn.addEventListener('click', () => {
      document.querySelectorAll('.nav-item').forEach(b => b.classList.remove('active'));
      btn.classList.add('active');
      selectedArtist = null;
      selectedAlbum = null;
      loadView(btn.dataset.view);
    });
  });

  async function loadView(view) {
    currentView = view;
    sidebarList.innerHTML = '';
    contentList.innerHTML = '<div class="loading">Loading...</div>';

    try {
      if (view === 'artists') {
        contentHeader.textContent = 'Artists';
        const artists = await api.getArtists();
        renderArtistsSidebar(artists);
        // Show albums grid for all
        const albums = await api.getAlbums();
        renderAlbumsGrid(albums);
      } else if (view === 'albums') {
        contentHeader.textContent = 'Albums';
        const albums = await api.getAlbums();
        renderAlbumsGrid(albums);
      } else if (view === 'tracks') {
        contentHeader.textContent = 'All Tracks';
        const result = await api.getTracks(0, 500);
        renderTrackList(result.tracks);
      } else if (view === 'search') {
        contentHeader.textContent = 'Search Results';
      } else if (view === 'artist-detail') {
        contentHeader.textContent = selectedArtist;
        const albums = await api.getAlbums(selectedArtist);
        renderAlbumsGrid(albums);
      } else if (view === 'album-detail') {
        contentHeader.textContent = selectedAlbum;
        const tracks = await api.getAlbumTracks(selectedAlbum);
        renderTrackList(tracks, true);
      }
    } catch (e) {
      contentList.innerHTML = `<div class="loading">Error loading: ${e.message}</div>`;
    }
  }

  // --- Render Functions ---

  function renderArtistsSidebar(artists) {
    sidebarList.innerHTML = '';
    artists.forEach(a => {
      const el = document.createElement('div');
      el.className = 'sidebar-item';
      el.innerHTML = `${esc(a.name)} <span class="count">${a.track_count}</span>`;
      el.addEventListener('click', () => {
        document.querySelectorAll('.sidebar-item').forEach(s => s.classList.remove('active'));
        el.classList.add('active');
        selectedArtist = a.name;
        loadView('artist-detail');
      });
      sidebarList.appendChild(el);
    });
  }

  function renderAlbumsGrid(albums) {
    contentList.innerHTML = '';
    if (albums.length === 0) {
      contentList.innerHTML = '<div class="loading">No albums found</div>';
      return;
    }
    const grid = document.createElement('div');
    grid.className = 'album-grid';

    albums.forEach(album => {
      const card = document.createElement('div');
      card.className = 'album-card';
      card.innerHTML = `
        <div class="album-art"><span>\u266B</span></div>
        <div class="album-name">${esc(album.name)}</div>
        <div class="album-artist-name">${esc(album.artist || '')}${album.year ? ' \u00B7 ' + album.year : ''}</div>
      `;
      card.addEventListener('click', () => {
        selectedAlbum = album.name;
        loadView('album-detail');
      });
      grid.appendChild(card);
    });

    contentList.appendChild(grid);
  }

  function renderTrackList(tracks, showNumbers = false) {
    contentList.innerHTML = '';
    if (tracks.length === 0) {
      contentList.innerHTML = '<div class="loading">No tracks found</div>';
      return;
    }

    tracks.forEach((track, idx) => {
      const row = document.createElement('div');
      row.className = 'track-row';
      if (queue.currentTrack && queue.currentTrack.id === track.id) {
        row.classList.add('playing');
      }

      const num = showNumbers && track.track_number ? track.track_number : idx + 1;
      row.innerHTML = `
        <span class="track-num">${num}</span>
        <div class="track-info">
          <div class="track-title">${esc(track.title || 'Unknown')}</div>
          <div class="track-meta">${esc(track.artist || '')}${track.album ? ' \u2014 ' + esc(track.album) : ''}</div>
        </div>
        <span class="track-duration">${formatTime(track.duration)}</span>
      `;

      row.addEventListener('click', () => {
        queue.setTracks(tracks, idx);
        playCurrentTrack();
      });

      contentList.appendChild(row);
    });
  }

  // --- Playback ---

  function playCurrentTrack() {
    const track = queue.currentTrack;
    if (!track) return;

    player.load(api.streamUrl(track.id));
    player.play();

    nowTitle.textContent = track.title || 'Unknown';
    nowArtist.textContent = track.artist || '';

    // Highlight playing track in list
    document.querySelectorAll('.track-row').forEach(row => row.classList.remove('playing'));
    // Find by index isn't reliable if list changed, but good enough for v1
    const rows = document.querySelectorAll('.track-row');
    if (rows[queue.currentIndex]) {
      rows[queue.currentIndex].classList.add('playing');
    }
  }

  // Transport controls
  btnPlay.addEventListener('click', () => player.togglePlay());
  $('btn-prev').addEventListener('click', () => {
    if (player.currentTime > 3) {
      player.seek(0);
    } else {
      queue.prev();
      playCurrentTrack();
    }
  });
  $('btn-next').addEventListener('click', () => {
    queue.next();
    playCurrentTrack();
  });

  // Progress
  player.on('timeupdate', (current, duration) => {
    if (duration && isFinite(duration)) {
      const pct = (current / duration) * 100;
      progressFill.style.width = pct + '%';
      progressSeek.value = pct;
      timeCurrent.textContent = formatTime(current);
      timeTotal.textContent = formatTime(duration);
    }
  });

  progressSeek.addEventListener('input', (e) => {
    player.seekPercent(parseFloat(e.target.value));
  });

  // Play/pause icon
  player.on('play', () => { btnPlay.textContent = '\u23F8'; });
  player.on('pause', () => { btnPlay.textContent = '\u25B6'; });

  // Auto-next
  player.on('ended', () => {
    const next = queue.next();
    if (next) playCurrentTrack();
  });

  // Volume
  volumeSlider.value = player.volume * 100;
  volumeSlider.addEventListener('input', (e) => {
    player.volume = parseInt(e.target.value) / 100;
  });

  // --- Search ---
  let searchTimeout;
  searchInput.addEventListener('input', () => {
    clearTimeout(searchTimeout);
    const q = searchInput.value.trim();
    if (!q) {
      loadView(currentView === 'search' ? 'artists' : currentView);
      return;
    }
    searchTimeout = setTimeout(async () => {
      contentHeader.textContent = `Search: "${q}"`;
      try {
        const tracks = await api.search(q);
        renderTrackList(tracks);
        currentView = 'search';
      } catch (e) {
        contentList.innerHTML = `<div class="loading">Search error: ${e.message}</div>`;
      }
    }, 300);
  });

  // --- Keyboard Shortcuts ---
  document.addEventListener('keydown', (e) => {
    // Don't capture when typing in inputs
    if (e.target.tagName === 'INPUT') return;

    if (e.code === 'Space') {
      e.preventDefault();
      player.togglePlay();
    } else if (e.code === 'ArrowLeft') {
      e.preventDefault();
      player.seek(player.currentTime - 10);
    } else if (e.code === 'ArrowRight') {
      e.preventDefault();
      player.seek(player.currentTime + 10);
    } else if (e.code === 'ArrowUp') {
      e.preventDefault();
      player.volume = player.volume + 0.05;
      volumeSlider.value = player.volume * 100;
    } else if (e.code === 'ArrowDown') {
      e.preventDefault();
      player.volume = player.volume - 0.05;
      volumeSlider.value = player.volume * 100;
    }
  });

  // --- Helpers ---
  function formatTime(seconds) {
    if (!seconds || !isFinite(seconds)) return '0:00';
    const mins = Math.floor(seconds / 60);
    const secs = Math.floor(seconds % 60);
    return `${mins}:${secs.toString().padStart(2, '0')}`;
  }

  function esc(str) {
    if (!str) return '';
    const div = document.createElement('div');
    div.textContent = str;
    return div.innerHTML;
  }

  // --- Init ---
  init().catch(e => {
    loadingScreen.innerHTML = `<div class="loading" style="color:var(--color-error,#f38ba8)">Error: ${e.message}</div>`;
  });
})();
