(function() {
  'use strict';

  // State
  let currentView = 'artists';
  let selectedArtist = null;
  let selectedAlbum = null;

  // DOM refs
  const $ = (id) => document.getElementById(id);
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
  const loadingScreen = $('loading-screen');

  // --- Init ---

  async function init() {
    try {
      const resp = await MusicAPI.init();
      if (resp.cached && MusicAPI.tracks.length > 0) {
        showMainUI();
      } else {
        loadingScreen.querySelector('.loading').textContent = 'Scanning music library...';
        await MusicAPI.scan();
        showMainUI();
      }
    } catch (e) {
      loadingScreen.innerHTML =
        '<div class="loading" style="color:var(--color-error,#f38ba8)">Error: ' + esc(e.message) + '</div>';
    }
  }

  function showMainUI() {
    loadingScreen.classList.add('hidden');
    mainUI.classList.remove('hidden');
    transport.classList.remove('hidden');
    $('dir-display').textContent = MusicAPI.musicDir;
    loadView('artists');
  }

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

  function loadView(view) {
    currentView = view;
    sidebarList.innerHTML = '';

    if (view === 'artists') {
      contentHeader.textContent = 'Artists';
      const artists = MusicAPI.getArtists();
      renderArtistsSidebar(artists);
      const albums = MusicAPI.getAlbums();
      renderAlbumsGrid(albums);
    } else if (view === 'albums') {
      contentHeader.textContent = 'Albums';
      const albums = MusicAPI.getAlbums();
      renderAlbumsGrid(albums);
    } else if (view === 'tracks') {
      contentHeader.textContent = 'All Tracks';
      renderTrackList(MusicAPI.tracks);
    } else if (view === 'artist-detail') {
      contentHeader.textContent = selectedArtist;
      const albums = MusicAPI.getAlbums(selectedArtist);
      renderAlbumsGrid(albums);
    } else if (view === 'album-detail') {
      contentHeader.textContent = selectedAlbum;
      const tracks = MusicAPI.getAlbumTracks(selectedAlbum);
      renderTrackList(tracks, true);
    }
  }

  // --- Render Functions ---

  function renderArtistsSidebar(artists) {
    sidebarList.innerHTML = '';
    artists.forEach(a => {
      const el = document.createElement('div');
      el.className = 'sidebar-item';
      el.innerHTML = esc(a.name) + ' <span class="count">' + a.track_count + '</span>';
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

      let artHtml;
      if (album.has_cover && album.cover_track) {
        artHtml = '<img class="album-art-img" src="' + esc(MusicAPI.artUrl(album.cover_track)) + '" alt="">';
      } else {
        artHtml = '<div class="album-art"><span>\u266B</span></div>';
      }

      card.innerHTML =
        artHtml +
        '<div class="album-name">' + esc(album.name) + '</div>' +
        '<div class="album-artist-name">' + esc(album.artist || '') +
        (album.year ? ' \u00B7 ' + album.year : '') + '</div>';

      card.addEventListener('click', () => {
        selectedAlbum = album.name;
        loadView('album-detail');
      });
      grid.appendChild(card);
    });

    contentList.appendChild(grid);
  }

  function renderTrackList(tracks, showNumbers) {
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
      row.innerHTML =
        '<span class="track-num">' + num + '</span>' +
        '<div class="track-info">' +
          '<div class="track-title">' + esc(track.title || 'Unknown') + '</div>' +
          '<div class="track-meta">' + esc(track.artist || '') +
          (track.album ? ' \u2014 ' + esc(track.album) : '') + '</div>' +
        '</div>' +
        '<span class="track-duration">' + formatTime(track.duration) + '</span>';

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

    player.load(MusicAPI.streamUrl(track));
    player.play();

    nowTitle.textContent = track.title || 'Unknown';
    nowArtist.textContent = track.artist || '';

    document.querySelectorAll('.track-row').forEach(row => row.classList.remove('playing'));
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

  player.on('play', () => { btnPlay.textContent = '\u23F8'; });
  player.on('pause', () => { btnPlay.textContent = '\u25B6'; });

  player.on('ended', () => {
    const next = queue.next();
    if (next) playCurrentTrack();
  });

  // Volume
  volumeSlider.value = player.volume * 100;
  volumeSlider.addEventListener('input', (e) => {
    player.volume = parseInt(e.target.value) / 100;
  });

  // --- Rescan ---

  $('btn-rescan').addEventListener('click', async () => {
    const btn = $('btn-rescan');
    btn.disabled = true;
    btn.textContent = 'Scanning...';
    try {
      await MusicAPI.scan();
      $('dir-display').textContent = MusicAPI.musicDir;
      loadView(currentView === 'search' ? 'artists' : currentView);
    } catch (e) {
      contentList.innerHTML = '<div class="loading">Scan error: ' + esc(e.message) + '</div>';
    }
    btn.disabled = false;
    btn.textContent = 'Rescan';
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
    searchTimeout = setTimeout(() => {
      contentHeader.textContent = 'Search: "' + esc(q) + '"';
      const tracks = MusicAPI.search(q);
      renderTrackList(tracks);
      currentView = 'search';
    }, 200);
  });

  // --- Keyboard Shortcuts ---

  document.addEventListener('keydown', (e) => {
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
    return mins + ':' + secs.toString().padStart(2, '0');
  }

  function esc(str) {
    if (!str) return '';
    const div = document.createElement('div');
    div.textContent = str;
    return div.innerHTML;
  }

  // --- Start ---
  init();
})();
