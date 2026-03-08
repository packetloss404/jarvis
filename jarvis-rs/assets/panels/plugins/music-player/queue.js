class Queue {
  constructor() {
    this.tracks = [];
    this.currentIndex = -1;
    this.shuffle = false;
    this.repeat = 'none'; // 'none', 'all', 'one'
    this._shuffleOrder = [];
    this._shuffleIndex = -1;
  }

  setTracks(tracks, startIndex = 0) {
    this.tracks = tracks;
    this.currentIndex = startIndex;
    this._rebuildShuffle();
    this.save();
  }

  get currentTrack() {
    if (this.currentIndex >= 0 && this.currentIndex < this.tracks.length) {
      return this.tracks[this.currentIndex];
    }
    return null;
  }

  next() {
    if (this.tracks.length === 0) return null;

    if (this.repeat === 'one') {
      return this.currentTrack;
    }

    if (this.shuffle) {
      this._shuffleIndex++;
      if (this._shuffleIndex >= this._shuffleOrder.length) {
        if (this.repeat === 'all') {
          this._rebuildShuffle();
          this._shuffleIndex = 0;
        } else {
          return null;
        }
      }
      this.currentIndex = this._shuffleOrder[this._shuffleIndex];
    } else {
      this.currentIndex++;
      if (this.currentIndex >= this.tracks.length) {
        if (this.repeat === 'all') {
          this.currentIndex = 0;
        } else {
          this.currentIndex = this.tracks.length - 1;
          return null;
        }
      }
    }

    this.save();
    return this.currentTrack;
  }

  prev() {
    if (this.tracks.length === 0) return null;

    if (this.shuffle) {
      this._shuffleIndex = Math.max(0, this._shuffleIndex - 1);
      this.currentIndex = this._shuffleOrder[this._shuffleIndex];
    } else {
      this.currentIndex = Math.max(0, this.currentIndex - 1);
    }

    this.save();
    return this.currentTrack;
  }

  _rebuildShuffle() {
    this._shuffleOrder = Array.from({ length: this.tracks.length }, (_, i) => i);
    // Fisher-Yates shuffle
    for (let i = this._shuffleOrder.length - 1; i > 0; i--) {
      const j = Math.floor(Math.random() * (i + 1));
      [this._shuffleOrder[i], this._shuffleOrder[j]] = [this._shuffleOrder[j], this._shuffleOrder[i]];
    }
    this._shuffleIndex = -1;
  }

  save() {
    try {
      const data = {
        trackIds: this.tracks.map(t => t.id),
        currentIndex: this.currentIndex,
        shuffle: this.shuffle,
        repeat: this.repeat,
      };
      localStorage.setItem('music-player:queue', JSON.stringify(data));
    } catch (e) {}
  }
}

const queue = new Queue();
