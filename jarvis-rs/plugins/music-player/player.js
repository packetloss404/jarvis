class AudioPlayer {
  constructor() {
    this.audio = new Audio();
    this.audio.preload = 'auto';
    this._onPlay = [];
    this._onPause = [];
    this._onEnded = [];
    this._onTimeUpdate = [];
    this._onError = [];

    this.audio.addEventListener('play', () => this._onPlay.forEach(fn => fn()));
    this.audio.addEventListener('pause', () => this._onPause.forEach(fn => fn()));
    this.audio.addEventListener('ended', () => this._onEnded.forEach(fn => fn()));
    this.audio.addEventListener('timeupdate', () => {
      this._onTimeUpdate.forEach(fn => fn(this.audio.currentTime, this.audio.duration));
    });
    this.audio.addEventListener('error', (e) => {
      console.error('Audio error:', e);
      this._onError.forEach(fn => fn(e));
    });

    // Restore volume
    const savedVol = localStorage.getItem('music-player:volume');
    if (savedVol !== null) {
      this.audio.volume = parseFloat(savedVol);
    } else {
      this.audio.volume = 0.8;
    }
  }

  load(url) {
    this.audio.src = url;
    this.audio.load();
  }

  play() {
    return this.audio.play();
  }

  pause() {
    this.audio.pause();
  }

  togglePlay() {
    if (this.audio.paused) {
      return this.play();
    } else {
      this.pause();
    }
  }

  seek(time) {
    if (isFinite(time)) {
      this.audio.currentTime = time;
    }
  }

  seekPercent(pct) {
    if (this.audio.duration && isFinite(this.audio.duration)) {
      this.audio.currentTime = (pct / 100) * this.audio.duration;
    }
  }

  get volume() {
    return this.audio.volume;
  }

  set volume(v) {
    this.audio.volume = Math.max(0, Math.min(1, v));
    localStorage.setItem('music-player:volume', this.audio.volume);
  }

  get currentTime() { return this.audio.currentTime; }
  get duration() { return this.audio.duration; }
  get paused() { return this.audio.paused; }

  on(event, fn) {
    const map = {
      play: this._onPlay,
      pause: this._onPause,
      ended: this._onEnded,
      timeupdate: this._onTimeUpdate,
      error: this._onError,
    };
    if (map[event]) map[event].push(fn);
  }
}

const player = new AudioPlayer();
