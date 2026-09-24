package dev.local.khinsider

import android.view.KeyEvent
import android.view.View
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
// IMPORTANT: TauriActivity attaches the WebView itself by calling `setContentView(webView)`
// *after* `onWebViewCreate` returns. An earlier version of this file tried to reparent the
// WebView into a SwipeRefreshLayout from inside onWebViewCreate -- that raced Tauri's own attach
// and crashed with "The specified child already has a parent." The fix is to intercept
// setContentView itself and wrap whatever it's given, instead of fighting the base class over
// who attaches the WebView.
class MainActivity : TauriActivity() {
    private lateinit var webView: WebView

    override fun onWebViewCreate(webView: WebView) {
        super.onWebViewCreate(webView)
        this.webView = webView
    }

    override fun setContentView(view: View) {
        if (view !== webView) {
            super.setContentView(view)
            return
        }

        val swipeRefresh = SwipeRefreshLayout(this).apply {
            addView(
                view,
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

        super.setContentView(swipeRefresh)
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
