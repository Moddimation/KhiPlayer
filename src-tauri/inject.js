// Runs inside every khinsider page (Tauri initialization_script).
// Reads the site's own player (#audio1) and forwards it to Rust -> Discord Rich Presence.
//
// Discord layout produced:
//   line 1 (bold)  track name
//   line 2         by Publisher                  (falls back to developer)
//   line 3         in Game (Platform)            ("(Platform)" is dropped when the site lists none)
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

  // ---- Mobile: auto-prompt a Discord login the first time we see audio play with no ----
  // ---- session connected, and keep the lockscreen/notification media session in sync. ----
  let discordPromptedOnce = false;
  let lastMediaSync = 0;
  function maybeConnectDiscord(paused) {
    if (paused || discordPromptedOnce) return;
    discordPromptedOnce = true;
    invoke('discord_connected').then((connected) => {
      // discord_connected only exists on mobile; on desktop invoke() above swallows the
      // "command not found" and resolves to undefined, so this stays a no-op there.
      if (connected === false) invoke('connect_discord');
    });
  }
  function syncMediaSession(audio, track, who, cover) {
    const now = Date.now();
    if (now - lastMediaSync < 800) return; // updateTimeline-worthy changes only, not every tick
    lastMediaSync = now;
    // plugin:<name>|<command> is Tauri v2's invoke naming for plugin commands; harmless no-op
    // on desktop (no such plugin registered there) and if the exact command name ever drifts
    // from the tauri-plugin-media-session version in Cargo.toml -- check `cargo doc` for that
    // crate if lockscreen art/controls stop updating after a crate bump.
    invoke('plugin:media-session|update_state', {
      title: track,
      artist: who || undefined,
      artworkUrl: cover,
      duration: isFinite(audio.duration) ? audio.duration : undefined,
      position: audio.currentTime,
      isPlaying: !audio.paused,
      canPrev: true,
      canNext: true,
    });
  }
  function clearMediaSession() { invoke('plugin:media-session|clear'); }

  // ---- Manual reload: Ctrl+R / Cmd+R (Bluetooth keyboards), and pull-down-to-refresh. ----
  // The Android WebView doesn't have a native swipe-refresh gesture on its own (that's usually
  // added natively via SwipeRefreshLayout -- see android-overlay/MainActivity.kt), but this
  // covers external keyboards and desktop/browser testing for free.
  addEventListener('keydown', (e) => {
    if ((e.ctrlKey || e.metaKey) && !e.shiftKey && !e.altKey && e.key.toLowerCase() === 'r') {
      e.preventDefault();
      location.reload();
    }
  }, true);
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
  // "Platforms: <a>DS</a><br>" -> ['DS']
  function platformsOf(doc) {
    for (const p of doc.querySelectorAll('#pageContent p')) {
      const nodes = [...p.childNodes];
      const i = nodes.findIndex((n) => n.nodeType === 3 && /Platforms?:/i.test(n.textContent));
      if (i < 0) continue;
      const out = [];
      for (let j = i + 1; j < nodes.length && nodes[j].nodeName !== 'BR'; j++) {
        if (nodes[j].nodeName === 'A') out.push(nodes[j].textContent.trim());
      }
      return out.filter(Boolean);
    }
    return [];
  }
  function parseAlbum(doc) {
    const img = doc.querySelector('#pageContent .albumImage img');
    return {
      title: doc.querySelector('#pageContent h2')?.textContent.trim() || '',
      publishers: names(doc, 'publisher'),
      developers: names(doc, 'developer'),
      platforms: platformsOf(doc),
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
      const platform = info?.platforms?.join(', ') || '';
      const cover = info?.cover
        || abs(document.querySelector(`a[href$="/album/${slug}"] img`)?.getAttribute('src'));

      const paused = audio.paused;
      const dur = audio.duration;
      const start = Date.now() - Math.round(audio.currentTime * 1000);
      // Build "by X" / "in Game (Platform)" while keeping the suffix inside Discord's 128-char limit
      const line = (prefix, name, suffix = '') =>
        prefix + name.slice(0, Math.max(0, 128 - prefix.length - suffix.length)) + suffix;
      const gameLine = album ? line('in ', album, platform ? ` (${platform})` : '') : '';
      const p = {
        details: clamp(track),
        state: clamp(who ? line('by ', who) : gameLine || 'KHInsider'),  // line 2
        imageText: who && gameLine ? clamp(gameLine) : undefined,        // line 3
        image: cover,
        url: slug ? `${ORIGIN}/game-soundtracks/album/${slug}` : undefined,
        paused,
        start: !paused && isFinite(dur) && dur > 0 ? start : undefined,
        end: !paused && isFinite(dur) && dur > 0 ? start + Math.round(dur * 1000) : undefined,
      };

      // Skip duplicates (play + playing fire back to back, etc.)
      const key = JSON.stringify([p.details, p.state, p.imageText, p.image, p.url, p.paused]);
      const near = (a, b) => (a == null && b == null) || (a != null && b != null && Math.abs(a - b) < 1500);
      const isDup = last && last.key === key && near(last.start, p.start) && near(last.end, p.end);
      if (!isDup) {
        last = { key, start: p.start, end: p.end };
        invoke('set_presence', { presence: p });
      }
      maybeConnectDiscord(paused);
      syncMediaSession(audio, track, who, cover);
    }

    function clearPresence() { seq++; last = null; invoke('clear_presence'); clearMediaSession(); }
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
