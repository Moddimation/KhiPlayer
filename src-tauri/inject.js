// Runs inside every khinsider page (Tauri initialization_script).
// Reads the site's own player (#audio1) and forwards it to Rust -> Discord Rich Presence.
//
// Discord layout produced:
//   line 1 (bold)  track name
//   line 2         publisher (e.g. "Nintendo")   <- falls back to developer, then album
//   line 3         album / game name (image hover text)
(() => {
  if (window.__khiPresence) return;
  if (location.hostname !== 'downloads.khinsider.com') return;
  window.__khiPresence = true;

  // 'publisher' -> "Nintendo" (falls back to developer if the site lists no publisher)
  // 'developer' -> developer only
  // 'both'      -> "Developer / Publisher" (collapsed to one name when they are identical)
  const CREDIT = 'publisher';

  const ORIGIN = location.origin;
  const invoke = (cmd, args) => {
    try { return window.__TAURI__.core.invoke(cmd, args).catch(() => {}); } catch (_) {}
  };
  const clamp = (s) => {
    s = String(s ?? '').replace(/\s+/g, ' ').trim().slice(0, 128);
    return s.length === 1 ? s + '\u200b' : s; // Discord requires >= 2 chars
  };
  const abs = (u) => { try { return new URL(u, location.href).href; } catch (_) { return undefined; } };
  const slugFromAudio = (a) => (a.currentSrc || a.src || '').match(/\/soundtracks\/([^/]+)\//)?.[1];
  const slugFromPath = () => location.pathname.match(/\/game-soundtracks\/album\/([^/]+)/)?.[1];
  const names = (doc, kind) => [...new Set(
    [...doc.querySelectorAll(`#pageContent a[href*="/game-soundtracks/${kind}/"]`)]
      .map((a) => a.textContent.trim()).filter(Boolean))];

  // Album pages carry the credits ("Developed by: / Published by:"). Playlist pages do not,
  // so we read them from the album page (same-origin fetch) and cache per album.
  function parseAlbum(doc) {
    const img = doc.querySelector('#pageContent .albumImage img');
    return {
      title: doc.querySelector('#pageContent h2')?.textContent.trim() || '',
      publishers: names(doc, 'publisher'),
      developers: names(doc, 'developer'),
      cover: img ? abs(img.getAttribute('src')) : undefined,
    };
  }
  const cache = new Map();
  function albumInfo(slug) {
    if (!slug) return Promise.resolve(null);
    if (!cache.has(slug)) {
      const p = slugFromPath() === slug && document.querySelector('#pageContent h2')
        ? Promise.resolve(parseAlbum(document))
        : fetch(`${ORIGIN}/game-soundtracks/album/${slug}`, { credentials: 'same-origin' })
            .then((r) => (r.ok ? r.text() : Promise.reject(r.status)))
            .then((html) => parseAlbum(new DOMParser().parseFromString(html, 'text/html')));
      cache.set(slug, p.catch(() => { cache.delete(slug); return null; }));
    }
    return cache.get(slug);
  }
  function credit(info) {
    if (!info) return '';
    const pub = info.publishers.join(', '), dev = info.developers.join(', ');
    if (CREDIT === 'developer') return dev || pub;
    if (CREDIT === 'both') return dev && pub && dev !== pub ? `${dev} / ${pub}` : pub || dev;
    return pub || dev;
  }

  function start() {
    const audio = document.getElementById('audio1');
    if (!audio) return;

    let played = false, seq = 0, timer = 0, last = null;

    async function sync() {
      const my = ++seq;
      if (audio.ended) return clearPresence();
      if (!played || !(audio.currentSrc || audio.src)) return;

      const slug = slugFromAudio(audio) || slugFromPath();
      const track = document.getElementById('audioplayerCurrentSong')?.textContent.trim() || 'Unknown track';
      const info = await albumInfo(slug);
      if (my !== seq) return; // a newer update superseded this one

      // Fallback when the album page could not be read: find the album link on this page.
      const album = info?.title
        || [...document.querySelectorAll(`a[href$="/album/${slug}"]`)].map((a) => a.textContent.trim()).find(Boolean)
        || '';
      const who = credit(info);
      const cover = info?.cover
        || abs(document.querySelector(`a[href$="/album/${slug}"] img`)?.getAttribute('src'));

      const paused = audio.paused;
      const dur = audio.duration;
      const start = Date.now() - Math.round(audio.currentTime * 1000);
      const p = {
        details: clamp(track),
        state: clamp(who || album || 'KHInsider'),        // line 2: publisher
        imageText: who && album ? clamp(album) : undefined, // line 3: game
        image: cover,
        url: slug ? `${ORIGIN}/game-soundtracks/album/${slug}` : undefined,
        paused,
        start: !paused && isFinite(dur) && dur > 0 ? start : undefined,
        end: !paused && isFinite(dur) && dur > 0 ? start + Math.round(dur * 1000) : undefined,
      };

      // Skip duplicates (play + playing fire back to back, etc.)
      const key = JSON.stringify([p.details, p.state, p.imageText, p.image, p.url, p.paused]);
      const near = (a, b) => (a == null && b == null) || (a != null && b != null && Math.abs(a - b) < 1500);
      if (last && last.key === key && near(last.start, p.start) && near(last.end, p.end)) return;
      last = { key, start: p.start, end: p.end };
      invoke('set_presence', { presence: p });
    }

    function clearPresence() { seq++; last = null; invoke('clear_presence'); }
    const schedule = () => { clearTimeout(timer); timer = setTimeout(sync, 200); };

    audio.addEventListener('play', () => { played = true; schedule(); });
    ['playing', 'pause', 'seeked', 'loadedmetadata', 'durationchange', 'ended'].forEach((e) =>
      audio.addEventListener(e, schedule));
    audio.addEventListener('emptied', schedule);

    // Track changes: the site swaps the title text and/or audio.src
    const title = document.getElementById('audioplayerCurrentSong');
    if (title) new MutationObserver(schedule).observe(title, { childList: true, characterData: true, subtree: true });
    new MutationObserver(schedule).observe(audio, { attributes: true, attributeFilter: ['src'] });

    addEventListener('pagehide', clearPresence);
  }

  if (document.readyState === 'loading') addEventListener('DOMContentLoaded', start);
  else start();
})();
