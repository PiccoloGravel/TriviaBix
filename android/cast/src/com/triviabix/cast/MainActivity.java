package com.triviabix.cast;

import android.app.Activity;
import android.app.AlertDialog;
import android.app.Presentation;
import android.content.ActivityNotFoundException;
import android.content.Context;
import android.content.Intent;
import android.content.SharedPreferences;
import android.graphics.Color;
import android.hardware.display.DisplayManager;
import android.net.Uri;
import android.os.Bundle;
import android.provider.Settings;
import android.text.InputType;
import android.view.Display;
import android.view.Gravity;
import android.view.View;
import android.view.WindowManager;
import android.webkit.JavascriptInterface;
import android.webkit.WebResourceError;
import android.webkit.WebResourceRequest;
import android.webkit.WebSettings;
import android.webkit.WebView;
import android.webkit.WebViewClient;
import android.widget.Button;
import android.widget.EditText;
import android.widget.LinearLayout;
import android.widget.TextView;

/**
 * The phone shows the player page; when the phone is cast to a TV (Smart View, or any screen
 * Android offers as a second display), the TV gets its own page, /tv, instead of a copy of the
 * phone. So one phone can put the game on the TV and still play.
 */
public class MainActivity extends Activity implements DisplayManager.DisplayListener {
    private static final String PREFS = "triviabix";
    private static final String KEY_SERVER = "server";
    private static final String DEFAULT_SERVER = "http://10.0.0.123:8080";

    private SharedPreferences prefs;
    private DisplayManager displays;
    private WebView phone;
    private TextView status;
    private TvScreen tv;
    private AlertDialog serverDialog;

    @Override
    protected void onCreate(Bundle saved) {
        super.onCreate(saved);
        prefs = getSharedPreferences(PREFS, MODE_PRIVATE);
        displays = (DisplayManager) getSystemService(Context.DISPLAY_SERVICE);
        getWindow().addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);
        getWindow().setStatusBarColor(Color.parseColor("#1A0B3D"));

        float dp = getResources().getDisplayMetrics().density;
        LinearLayout root = new LinearLayout(this);
        root.setOrientation(LinearLayout.VERTICAL);
        root.setBackgroundColor(Color.parseColor("#1A0B3D"));
        // The status bar is drawn over the page from Android 15, so keep the page below it.
        root.setFitsSystemWindows(true);

        LinearLayout bar = new LinearLayout(this);
        bar.setGravity(Gravity.CENTER_VERTICAL);
        bar.setPadding((int) (12 * dp), (int) (4 * dp), (int) (4 * dp), (int) (4 * dp));
        status = new TextView(this);
        status.setTextColor(Color.WHITE);
        status.setTextSize(14);
        status.setOnClickListener(v -> showCastHelp());
        bar.addView(status, new LinearLayout.LayoutParams(0, LinearLayout.LayoutParams.WRAP_CONTENT, 1));
        bar.addView(button("📺 Cast", v -> showCastHelp()));
        bar.addView(button("⚙", v -> askServer(false)));
        root.addView(bar);

        phone = new WebView(this);
        setUp(phone);
        phone.addJavascriptInterface(new Bridge(), "TriviaBixApp");
        phone.setWebViewClient(new Client() {
            @Override
            public void onReceivedError(WebView view, WebResourceRequest req, WebResourceError err) {
                if (req.isForMainFrame()) askServer(true);
            }
        });
        root.addView(phone, new LinearLayout.LayoutParams(LinearLayout.LayoutParams.MATCH_PARENT, 0, 1));
        setContentView(root);

        if (prefs.contains(KEY_SERVER)) phone.loadUrl(server() + "/");
        else askServer(false);
        showStatus();
    }

    private Button button(String label, View.OnClickListener onClick) {
        Button b = new Button(this);
        b.setText(label);
        b.setAllCaps(false);
        b.setOnClickListener(onClick);
        return b;
    }

    @Override
    protected void onStart() {
        super.onStart();
        displays.registerDisplayListener(this, null);
        updateTv();
    }

    @Override
    protected void onDestroy() {
        displays.unregisterDisplayListener(this);
        closeTv();
        phone.destroy();
        super.onDestroy();
    }

    @Override
    public void onBackPressed() {
        if (phone.canGoBack()) phone.goBack();
        else super.onBackPressed();
    }

    @Override public void onDisplayAdded(int id) { updateTv(); }
    @Override public void onDisplayRemoved(int id) { updateTv(); }
    @Override public void onDisplayChanged(int id) { updateTv(); }

    /** Shows /tv on the first second screen, or takes it down when the cast ends. */
    private void updateTv() {
        Display[] screens = displays.getDisplays(DisplayManager.DISPLAY_CATEGORY_PRESENTATION);
        Display target = screens.length > 0 ? screens[0] : null;
        if (tv != null && (target == null || tv.getDisplay().getDisplayId() != target.getDisplayId())) {
            closeTv();
        }
        if (target != null && tv == null && prefs.contains(KEY_SERVER)) {
            tv = new TvScreen(this, target, server() + "/tv");
            tv.setOnDismissListener(d -> { if (tv == d) { tv = null; showStatus(); } });
            try {
                tv.show();
            } catch (WindowManager.InvalidDisplayException e) {
                tv = null; // the screen went away while we were opening it
            }
        }
        showStatus();
    }

    private void closeTv() {
        if (tv == null) return;
        TvScreen old = tv;
        tv = null;
        old.dismiss();
    }

    private void showStatus() {
        status.setText(tv != null
                ? "📺 On the TV: " + tv.getDisplay().getName()
                : "📺 Not on a TV yet. Tap Cast.");
    }

    private String server() {
        return prefs.getString(KEY_SERVER, DEFAULT_SERVER);
    }

    private void askServer(boolean failed) {
        if (serverDialog != null && serverDialog.isShowing()) return;
        EditText input = new EditText(this);
        input.setInputType(InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_VARIATION_URI);
        input.setText(server());
        input.setSelectAllOnFocus(true);
        serverDialog = new AlertDialog.Builder(this)
                .setTitle(failed ? "Can't reach the game" : "Game server address")
                .setMessage(failed
                        ? "Check the phone is on the home Wi-Fi and the address is right."
                        : "The address you open TriviaBix at, like " + DEFAULT_SERVER)
                .setView(input)
                .setPositiveButton(failed ? "Try again" : "Save", (d, w) -> {
                    String url = input.getText().toString().trim().replaceAll("/+$", "");
                    if (url.isEmpty()) url = DEFAULT_SERVER;
                    if (!url.matches("(?i)^https?://.*")) url = "http://" + url;
                    prefs.edit().putString(KEY_SERVER, url).apply();
                    phone.loadUrl(url + "/");
                    closeTv();
                    updateTv();
                })
                .setCancelable(prefs.contains(KEY_SERVER))
                .show();
    }

    private void showCastHelp() {
        new AlertDialog.Builder(this)
                .setTitle("Put the game on the TV")
                .setMessage("1. Swipe down from the top of the screen twice and tap Smart View.\n"
                        + "2. Pick your TV. Choose to show the phone screen on the TV if it asks.\n"
                        + "3. Come back here. The TV shows the game, and this phone stays your controller.\n\n"
                        + "If the TV just copies this phone instead, Smart View on that TV can't show a "
                        + "second screen. Open " + server() + "/tv in the TV's browser instead.")
                .setPositiveButton("Open cast settings", (d, w) -> openCastSettings())
                .setNegativeButton("Close", null)
                .show();
    }

    private void openCastSettings() {
        try {
            startActivity(new Intent(Settings.ACTION_CAST_SETTINGS));
        } catch (ActivityNotFoundException e) {
            // Samsung keeps Smart View in the quick panel only; the steps above still apply.
        }
    }

    static void setUp(WebView view) {
        WebSettings s = view.getSettings();
        s.setJavaScriptEnabled(true);
        s.setDomStorageEnabled(true);
        // The host's voice and the music start on their own, with nobody to tap the TV.
        s.setMediaPlaybackRequiresUserGesture(false);
        s.setUserAgentString(s.getUserAgentString() + " TriviaBixCast/1");
        view.setBackgroundColor(Color.parseColor("#1A0B3D"));
    }

    /** Keeps the game's own pages in the app; anything else (such as Apple Music) opens outside. */
    private class Client extends WebViewClient {
        @Override
        public boolean shouldOverrideUrlLoading(WebView view, WebResourceRequest req) {
            Uri url = req.getUrl();
            Uri home = Uri.parse(server());
            if (url.getHost() != null && url.getHost().equals(home.getHost()) && url.getPort() == home.getPort()) {
                return false;
            }
            try {
                startActivity(new Intent(Intent.ACTION_VIEW, url));
            } catch (ActivityNotFoundException e) {
                // nothing can open it; stay put
            }
            return true;
        }
    }

    /** Lets the phone page's "Put the game on the TV" button open the cast steps. */
    private class Bridge {
        @JavascriptInterface
        public void cast() {
            runOnUiThread(MainActivity.this::showCastHelp);
        }

        @JavascriptInterface
        public boolean onTv() {
            return tv != null;
        }
    }

    /** The TV's screen: the /tv page, full screen, kept awake. */
    static class TvScreen extends Presentation {
        private final String url;
        private WebView view;

        TvScreen(Context context, Display display, String url) {
            super(context, display);
            this.url = url;
        }

        @Override
        protected void onCreate(Bundle saved) {
            super.onCreate(saved);
            getWindow().addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);
            view = new WebView(getContext());
            setUp(view);
            view.setWebViewClient(new WebViewClient());
            view.loadUrl(url);
            setContentView(view);
        }

        @Override
        protected void onStop() {
            if (view != null) view.destroy();
            view = null;
            super.onStop();
        }
    }
}
