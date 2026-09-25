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

  // ---- In-page "now playing" overlay (bottom-right, translucent). Lives in a shadow root so ----
  // ---- none of the site's CSS can touch it and none of ours leaks onto the site. ----
  function buildOverlay() {
    const host = document.createElement('div');
    host.id = 'khi-overlay-host';
    Object.assign(host.style, { position: 'fixed', inset: 'auto 16px 16px auto', zIndex: 2147483647 });
    const root = host.attachShadow({ mode: 'closed' });
    root.innerHTML = `
      <style>
        .card { display:flex; align-items:center; gap:10px; width:280px; padding:10px 12px;
          background:rgba(24,24,28,.72); backdrop-filter:blur(10px); border:1px solid rgba(255,255,255,.08);
          border-radius:14px; box-shadow:0 6px 24px rgba(0,0,0,.35); color:#eee;
          font:12px/1.35 -apple-system,Segoe UI,Roboto,sans-serif; transition:opacity .15s; }
        .card.collapsed .body { display:none; }
        img.art { width:40px; height:40px; border-radius:8px; object-fit:cover; background:#333; flex:none; }
        .meta { flex:1; min-width:0; }
        .title { font-weight:600; white-space:nowrap; overflow:hidden; text-overflow:ellipsis; }
        .artist { opacity:.65; white-space:nowrap; overflow:hidden; text-overflow:ellipsis; }
        .row { display:flex; align-items:center; gap:6px; margin-top:2px; }
        button { background:none; border:none; color:#eee; opacity:.85; cursor:pointer; padding:2px;
          font-size:15px; line-height:1; }
        button:hover { opacity:1; }
        input[type=range] { flex:1; height:3px; accent-color:#eee; }
        .toggle { position:absolute; top:-8px; right:-8px; width:18px; height:18px; border-radius:50%;
          background:rgba(24,24,28,.85); font-size:11px; display:flex; align-items:center; justify-content:center; }
      </style>
      <div class="card" style="position:relative">
        <button class="toggle" title="Ein-/ausblenden">-</button>
        <img class="art" />
        <div class="body meta">
          <div class="title">Nothing playing</div>
          <div class="artist"></div>
          <div class="row">
            <button class="prev" title="Zurück">⏮</button>
            <button class="play" title="Play/Pause">▶</button>
            <button class="next" title="Weiter">⏭</button>
            <input class="seek" type="range" min="0" max="1000" value="0" />
          </div>
        </div>
      </div>`;
    document.documentElement.appendChild(host);

    const $ = (s) => root.querySelector(s);
    const card = $('.card');
    $('.toggle').onclick = () => card.classList.toggle('collapsed');

    let seeking = false;
    $('.seek').addEventListener('input', (e) => { seeking = true; });
    $('.seek').addEventListener('change', (e) => {
      seeking = false;
      const frac = Number(e.target.value) / 1000;
      overlayControl('seek', frac * (localAudio?.duration || 0));
    });
    $('.play').onclick = () => overlayControl('playPause');
    $('.prev').onclick = () => overlayControl('prev');
    $('.next').onclick = () => overlayControl('next');

    return {
      setMeta(m) {
        $('.title').textContent = m.title || 'Nothing playing';
        $('.artist').textContent = m.artist || '';
        $('.art').src = m.artwork || '';
        $('.play').textContent = m.paused ? '▶' : '⏸';
      },
      setProgress(frac) { if (!seeking) $('.seek').value = String(Math.round((frac || 0) * 1000)); },
    };
  }
  const overlay = buildOverlay();

  // `localAudio` and `played` are declared here (not inside start()) on purpose -- an earlier
  // version had `played` as a local inside start(), which meant this check below was always
  // silently false and every control always went to the background player, even on a page that
  // had real, live audio right there. `played` becomes true the moment this page's own <audio>
  // genuinely starts playing (see start()); until then, or on a page with no player at all,
  // there's nothing local to control, so overlayControl below falls through to the background.
  let localAudio = null;
  let played = false;
  let stoppedBackground = false;
  let lastMeta = { title: 'Nothing playing', artist: '', artwork: '', paused: true };

  function showMeta(patch) {
    lastMeta = { ...lastMeta, ...patch };
    overlay.setMeta(lastMeta);
  }

  function overlayControl(action, value) {
    const localIsReal = localAudio && played;
    if (action === 'next' || action === 'prev') {
      if (localIsReal) clickSiteButton(action);
      return; // no equivalent once handed off to the background player
    }
    if (localIsReal) {
      // Same element the site's own bar uses -- no separate copy to fall out of sync with.
      if (action === 'playPause') { localAudio.paused ? localAudio.play().catch(() => {}) : localAudio.pause(); }
      else if (action === 'seek') localAudio.currentTime = value;
      else if (action === 'volume') localAudio.volume = value;
    } else {
      if (action === 'playPause') showMeta({ paused: !lastMeta.paused }); // no local event loop to feed this back
      invoke('player_control', { action, value });
    }
  }
  // Best-effort guess at the site's own prev/next buttons -- inspect the real page and adjust
  // this selector list if it doesn't find them (Discord presence/audio reading doesn't depend on
  // this at all, so getting it wrong only means the overlay's prev/next buttons do nothing).
  function clickSiteButton(which) {
    const patterns = which === 'next'
      ? ['#audioplayerNext', '.audioplayerNext', '[onclick*="ext" i]', '[title*="ext" i]', '[aria-label*="ext" i]']
      : ['#audioplayerPrevious', '.audioplayerPrevious', '[onclick*="rev" i]', '[title*="rev" i]', '[aria-label*="rev" i]'];
    for (const sel of patterns) {
      const el = document.querySelector(sel);
      if (el) { el.click(); return; }
    }
  }

  // Backfill immediately on load (covers "opened a page where nothing plays, but something is
  // still going from before"), then stay live for as long as this page is open.
  invoke('get_now_playing').then((m) => { if (m) showMeta(m); });
  window.__TAURI__.event.listen('now-playing-meta', (e) => showMeta(e.payload));
  // Mobile lockscreen (or any other external caller of player_control) landing here. Only
  // relevant when THIS page's own audio is the real, live source -- if it's the background
  // player instead, Rust already applied the action directly (see player_control in lib.rs), no
  // JS round-trip needed for that case.
  window.__TAURI__.event.listen('player-control', (e) => {
    const { action, value } = e.payload;
    const localIsReal = localAudio && played;
    if (action === 'next' || action === 'prev') { if (localIsReal) clickSiteButton(action); return; }
    if (!localIsReal) return;
    if (action === 'playPause') { localAudio.paused ? localAudio.play().catch(() => {}) : localAudio.pause(); }
    else if (action === 'seek' && typeof value === 'number') localAudio.currentTime = value;
    else if (action === 'volume' && typeof value === 'number') localAudio.volume = value;
  });

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
    localAudio = audio; // real, audible source for as long as this page is open -- see below

    let seq = 0, timer = 0, last = null;

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

      // This page's own audio is the single, real, audible source (overlayControl above talks
      // to it directly) -- so just reflect its actual live state, same as the Discord presence
      // right above. No gating on "is this a new track": every real change here is real.
      showMeta({ title: track, artist: who, artwork: cover, paused });
      invoke('report_now_playing_meta', { meta: { title: track, artist: who, artwork: cover, paused } });

      // The moment this page's own audio has genuinely started playing for real, make sure
      // nothing is left over-lapping from a previous page's handoff (see pagehide below).
      if (!stoppedBackground) {
        stoppedBackground = true;
        invoke('stop_background_audio');
      }
    }

    function clearPresence() { seq++; last = null; invoke('clear_presence'); clearMediaSession(); }
    const schedule = () => { clearTimeout(timer); timer = setTimeout(sync, 200); };

    audio.addEventListener('play', () => { played = true; schedule(); });
    ['playing', 'pause', 'seeked', 'loadedmetadata', 'durationchange', 'ended'].forEach((e) =>
      audio.addEventListener(e, schedule));
    audio.addEventListener('emptied', schedule);
    audio.addEventListener('timeupdate', () => {
      if (isFinite(audio.duration) && audio.duration > 0) overlay.setProgress(audio.currentTime / audio.duration);
    });

    // Track changes: the site swaps the title text and/or audio.src
    const title = document.getElementById('audioplayerCurrentSong');
    if (title) new MutationObserver(schedule).observe(title, { childList: true, characterData: true, subtree: true });
    new MutationObserver(schedule).observe(audio, { attributes: true, attributeFilter: ['src'] });

    // switch tearing this <audio> out without one). If it was actually playing, hand the exact
    // position off to the background player so the song keeps going wherever the user ends up
    // next; stop_background_audio above cancels this the moment a next page's own real audio
    // actually starts.
    //
    // Three separate triggers, because relying on just one silently loses the handoff:
    //  - `pagehide` fires only for a real browser navigation, AND it fires as the page is
    //    already being torn down -- invoke() is async IPC to Rust, so racing it against pagehide
    //    can lose the message entirely (unload handlers historically have this exact problem,
    //    which is why sendBeacon exists for analytics). Kept as a fallback, not the main path.
    //  - A capturing click listener on internal links fires BEFORE the browser starts navigating
    //    at all, giving the invoke a full navigation's worth of time to actually arrive. This is
    //    the main path for a real link click.
    //  - A MutationObserver catches this exact <audio> node being removed from the document
    //    without any real navigation at all (this site may swap sections via in-page JS) --
    //    pagehide never fires for that case, so without this the handoff just never happens.
    let handedOff = false;
    function doHandoff() {
      if (handedOff) return;
      const src = audio.currentSrc || audio.src;
      if (!audio.paused && !audio.ended && src) {
        handedOff = true;
        invoke('report_playback', { info: { src, position: audio.currentTime, volume: audio.volume } });
      }
    }

    document.addEventListener('click', (e) => {
      const a = e.target.closest?.('a[href]');
      if (a && !a.getAttribute('href').startsWith('#')) doHandoff();
    }, true);

    new MutationObserver(() => { if (!audio.isConnected) doHandoff(); })
      .observe(document.documentElement, { childList: true, subtree: true });

    addEventListener('pagehide', () => { doHandoff(); clearPresence(); });
  }

  if (document.readyState === 'loading') addEventListener('DOMContentLoaded', start);
  else start();
})();
