package dev.local.khinsider

import android.view.KeyEvent
import android.view.ViewGroup
import android.webkit.WebView
import androidx.swiperefreshlayout.widget.SwipeRefreshLayout

// Swatinem/rust-cache and `cargo tauri android init` regenerate everything else in this folder
// from scratch, but leave an existing MainActivity.kt alone if one is already there -- which is
// why this file has to be copied into place *after* init (see scripts/patch-android.sh and the
// "Patch native Android sources" CI step) rather than committed as part of the Rust source tree.
//
// Two things bolted on here that don't have a Tauri/JS-side equivalent:
//   1. Pull-down-to-refresh, wrapping the WebView in a SwipeRefreshLayout.
//   2. Ctrl+R on a physical/Bluetooth keyboard also reloads (inject.js already handles this via
//      a JS keydown listener, but that only fires once the page has finished loading -- this
//      covers it before/while that isn't true, e.g. if the page is stuck loading).
//
// IMPORTANT, two dead ends already tried here, so a third attempt doesn't repeat them:
//   1. Reparenting the WebView into a SwipeRefreshLayout directly inside onWebViewCreate crashed
//      with "The specified child already has a parent" -- TauriActivity calls
//      setContentView(webView) itself right after onWebViewCreate returns, and that collided
//      with our own reparenting.
//   2. Overriding setContentView(view: View) to intercept that call doesn't even compile
//      ("'setContentView' overrides nothing") -- TauriActivity (or something in its hierarchy)
//      apparently redeclares it without Kotlin's `open`, which hides the Android SDK's own open
//      method from being overridden here.
// The fix that actually works: do nothing in onWebViewCreate except grab the reference, then
// `post {}` the reparenting so it runs *after* the current call stack -- including Tauri's own
// setContentView(webView) -- has already finished. By the time our posted block runs, the
// WebView has a real parent we can safely detach it from.
class MainActivity : TauriActivity() {
    private lateinit var webView: WebView

    override fun onWebViewCreate(webView: WebView) {
        super.onWebViewCreate(webView)
        this.webView = webView
        // Same class of problem as desktop had (see background_audio in lib.rs) for the hidden
        // "player" window: audio in a WebView nobody ever taps in gets blocked by default. Unlike
        // desktop, Android has one official, documented setting for exactly this -- so unlike
        // desktop this one's fine to just flip, no native-audio rewrite needed here.
        webView.settings.mediaPlaybackRequiresUserGesture = false
        webView.post { wrapInSwipeRefresh(webView) }
    }

    private fun wrapInSwipeRefresh(webView: WebView) {
        val parent = webView.parent as? ViewGroup ?: return
        parent.removeView(webView)

        val swipeRefresh = SwipeRefreshLayout(this).apply {
            addView(
                webView,
                ViewGroup.LayoutParams(
                    ViewGroup.LayoutParams.MATCH_PARENT,
                    ViewGroup.LayoutParams.MATCH_PARENT,
                ),
            )
            setOnRefreshListener { webView.reload() }
        }

        // Only let the user pull-to-refresh when already scrolled to the top -- otherwise a
        // swipe-down mid-page would fight normal scrolling.
        webView.viewTreeObserver.addOnScrollChangedListener {
            swipeRefresh.isEnabled = webView.scrollY == 0
        }
        stopRefreshingOnLoad(swipeRefresh, webView)

        setContentView(swipeRefresh)
    }

    private fun stopRefreshingOnLoad(swipeRefresh: SwipeRefreshLayout, webView: WebView) {
        // Tauri installs its own WebViewClient (for the custom-protocol asset scheme and
        // navigation events); we don't want to replace it, just piggyback on page-finished via a
        // lightweight poll instead of fighting over WebViewClient ownership.
        val handler = android.os.Handler(mainLooper)
        val stopWhenIdle = object : Runnable {
            override fun run() {
                if (webView.progress >= 100) {
                    swipeRefresh.isRefreshing = false
                }
                handler.postDelayed(this, 150)
            }
        }
        handler.post(stopWhenIdle)
    }

    override fun dispatchKeyEvent(event: KeyEvent): Boolean {
        val ctrlR = event.isCtrlPressed && event.keyCode == KeyEvent.KEYCODE_R
        if (ctrlR && event.action == KeyEvent.ACTION_DOWN) {
            webView.reload()
            return true
        }
        return super.dispatchKeyEvent(event)
    }
}
