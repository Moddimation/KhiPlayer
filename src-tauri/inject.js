// Runs on every downloads.khinsider.com page (Tauri initialization script).
// Supports two page types (verified against saved pages):
//   album page: <audio id="audio1">, track in #audioplayerCurrentSong, cover in .albumImage img
//   track page: <audio id="audio">,  "Song name: <b>..</b>" in #pageContent, cover fetched from the album link
(() => {
  if (window.__khiRpc || !window.__TAURI__ || !window.__TAURI__.core) return;
  window.__khiRpc = true;

  const invoke = (cmd, args) => {
    try { return window.__TAURI__.core.invoke(cmd, args).catch(() => {}); } catch (_) {}
  };

  const DEBOUNCE_MS = 800;              // Discord rate-limits presence updates
  const CLEAR_AFTER_PAUSE_MS = 10 * 60e3;
  const txt = (el) => (el && el.textContent ? el.textContent.trim() : '');

  function readAlbumMeta(doc) {
    const h2 = doc.querySelector('h2');
    const img = doc.querySelector('.albumImage img');
    const pub = doc.querySelector('a[href*="/game-soundtracks/publisher/"]');
    return {
      title: txt(h2) || (doc.title || '').trim(),
      thumb: img ? img.src : '',
      publisher: txt(pub),
    };
  }

  function trackPageSong() {
    const p = [...document.querySelectorAll('#pageContent p')].find((x) => /Song name:/i.test(x.textContent));
    if (!p) return '';
    const bs = p.querySelectorAll('b');
    return txt(bs.length > 1 ? bs[1] : bs[0]);
  }

  function init() {
    invoke('clear_presence');           // new page => drop stale presence

    const albumAudio = document.getElementById('audio1');
    // fall back to any <audio> so unseen page types (e.g. playlists) still work
    const audio = albumAudio || document.getElementById('audio') || document.querySelector('audio');
    if (!audio) return;                 // home/search/etc. have no player

    const isAlbumPage = !!albumAudio;
    const trackEl = document.getElementById('audioplayerCurrentSong');
    const getTrack = () =>
      txt(trackEl) ||
      trackPageSong() ||
      txt(document.querySelector('#songlist tr.plSel td.clickable-row a')) ||
      '';

    let meta = readAlbumMeta(document);
    if (!isAlbumPage) {
      // track pages have no cover: read it once from the album page (same origin)
      const a = document.querySelector('#pageContent a[href^="/game-soundtracks/album/"]');
      if (a) {
        fetch(a.href, { credentials: 'include' })
          .then((r) => r.text())
          .then((html) => {
            const m = readAlbumMeta(new DOMParser().parseFromString(html, 'text/html'));
            meta = { title: meta.title || m.title, thumb: m.thumb, publisher: m.publisher };
          })
          .catch(() => {});
      }
    }

    let started = false, timer = null, pauseTimer = null;

    function push() {
      // playlist pages mix albums, so album name + cover change per track and live in the player itself
      const curAlbum = txt(document.getElementById('audioplayerCurrentlyPlayingAlbumName')) || meta.title;
      const curImg = document.querySelector('#audioplayerCurrentlyPlayingSongImage img');
      const curThumb = (curImg && curImg.src) || meta.thumb;
      const track = getTrack();
      if (!started || !track) return;

      try {
        if ('mediaSession' in navigator && window.MediaMetadata) {
          navigator.mediaSession.metadata = new MediaMetadata({
            title: track, artist: meta.publisher, album: curAlbum,
            artwork: curThumb ? [{ src: curThumb }] : [],
          });
        }
      } catch (_) {}

      const presence = {
        details: track,
        state: curAlbum,
        image: curThumb || null,
        imageText: curAlbum,
        paused: audio.paused,
        url: location.origin + location.pathname,
        start: null,
        end: null,
      };

      clearTimeout(pauseTimer);
      if (audio.paused) {
        pauseTimer = setTimeout(() => invoke('clear_presence'), CLEAR_AFTER_PAUSE_MS);
      } else {
        // Discord wants Unix milliseconds; start+end together give the Spotify-style time bar
        const start = Math.floor(Date.now() - audio.currentTime * 1000);
        presence.start = start;
        if (Number.isFinite(audio.duration) && audio.duration > 0) {
          presence.end = Math.floor(start + audio.duration * 1000);
        }
      }
      invoke('set_presence', { presence });
    }

    const schedule = () => { clearTimeout(timer); timer = setTimeout(push, DEBOUNCE_MS); };

    audio.addEventListener('playing', () => { started = true; schedule(); });
    ['pause', 'seeked', 'durationchange', 'ended'].forEach((e) => audio.addEventListener(e, schedule));
    if (trackEl) {
      new MutationObserver(schedule).observe(trackEl, { childList: true, characterData: true, subtree: true });
    }
  }

  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', init);
  else init();
})();
