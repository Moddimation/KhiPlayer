package dev.local.khinsider

import android.view.KeyEvent
import android.view.ViewGroup
import android.webkit.WebView
import androidx.swiperefreshlayout.widget.SwipeRefreshLayout
import app.tauri.plugin.PluginManager

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
class MainActivity : TauriActivity() {
    private lateinit var webView: WebView

    override fun onWebViewCreate(webView: WebView) {
        super.onWebViewCreate(webView)
        this.webView = webView

        // The WebView is already attached to the activity's content view by this point; pull it
        // out and re-host it inside a SwipeRefreshLayout instead.
        val parent = webView.parent as? ViewGroup
        val index = parent?.indexOfChild(webView) ?: -1
        val originalLayoutParams = webView.layoutParams
        parent?.removeView(webView)

        val swipeRefresh = SwipeRefreshLayout(this).apply {
            addView(
                webView,
                ViewGroup.LayoutParams(
                    ViewGroup.LayoutParams.MATCH_PARENT,
                    ViewGroup.LayoutParams.MATCH_PARENT,
                ),
            )
            setOnRefreshListener {
                webView.reload()
            }
        }

        if (parent != null && index >= 0) {
            parent.addView(swipeRefresh, index, originalLayoutParams)
        } else {
            // Fallback: no known parent (shouldn't normally happen) -- just make it the activity's
            // content view outright.
            setContentView(swipeRefresh)
        }

        // Android only calls WebView.onScrollChanged for the swipe indicator's own drag detection
        // once it knows whether the page is scrolled to the top; wire that up explicitly.
        webView.viewTreeObserver.addOnScrollChangedListener {
            swipeRefresh.isEnabled = webView.scrollY == 0
        }

        // Stop the spinner once the page (or an audio-driven re-render) has actually reloaded.
        stopRefreshingOnLoad(swipeRefresh, webView)
    }

    private fun stopRefreshingOnLoad(swipeRefresh: SwipeRefreshLayout, webView: WebView) {
        // Tauri installs its own WebViewClient (RustWebViewClient) to serve the custom-protocol
        // asset scheme and to wire up navigation events; we don't want to replace it (that would
        // break the app), just piggyback on page-finished via a lightweight poll instead of
        // fighting over WebViewClient ownership.
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
