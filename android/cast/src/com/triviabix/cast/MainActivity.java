package com.triviabix.cast;

import android.app.Activity;
import android.app.AlertDialog;
import android.app.Presentation;
import android.app.UiModeManager;
import android.content.ActivityNotFoundException;
import android.content.Context;
import android.content.Intent;
import android.content.SharedPreferences;
import android.content.pm.ActivityInfo;
import android.content.pm.PackageManager;
import android.content.res.Configuration;
import android.graphics.Color;
import android.hardware.display.DisplayManager;
import android.net.Uri;
import android.os.Bundle;
import android.provider.Settings;
import android.text.InputType;
import android.view.Display;
import android.view.Gravity;
import android.view.KeyEvent;
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

import java.util.List;

/**
 * One app for both ends of the game.
 *
 * On a phone it shows the player page; when the phone is cast to a TV (Smart View, or any screen
 * Android offers as a second display), the TV gets its own page, /tv, instead of a copy of the
 * phone. If the TV can only mirror the phone, "Be the TV" turns the phone itself into the TV
 * screen (sideways, full screen) and everyone plays on their own phones.
 *
 * On a TV stick (Fire TV, Google TV) it opens straight into /tv, full screen, with sound and no
 * screensaver, and the remote's OK button works like Enter on a keyboard.
 */
public class MainActivity extends Activity implements DisplayManager.DisplayListener {
    private static final String PREFS = "triviabix";
    private static final String KEY_SERVER = "server";
    /** The game's public address (Cloudflare Tunnel). Works at home and away. */
    static final String DEFAULT_SERVER = "https://trivia.jegan.online";
    /** Addresses older versions saved that no longer run the game; moved to DEFAULT_SERVER. */
    private static final String[] OLD_SERVERS = { "http://10.0.0.123:8080" };
    private static final int BG = Color.parseColor("#1A0B3D");

    private SharedPreferences prefs;
    private DisplayManager displays;
    private WebView phone;
    private View bar;
    private TextView status;
    private Button beTvButton;
    private TvScreen tv;
    private AlertDialog dialog;
    private ServerFinder finder;
    /** This device is a TV (Fire TV, Android TV): it only ever shows /tv. */
    private boolean isTv;
    /** The page on this screen is the TV page, on a TV or a phone that became the TV. */
    private boolean showingTvPage;

    @Override
    protected void onCreate(Bundle saved) {
        super.onCreate(saved);
        prefs = getSharedPreferences(PREFS, MODE_PRIVATE);
        displays = (DisplayManager) getSystemService(Context.DISPLAY_SERVICE);
        isTv = isTvDevice();
        getWindow().addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);
        getWindow().setStatusBarColor(BG);

        float dp = getResources().getDisplayMetrics().density;
        LinearLayout root = new LinearLayout(this);
        root.setOrientation(LinearLayout.VERTICAL);
        root.setBackgroundColor(BG);
        // The status bar is drawn over the page from Android 15, so keep the page below it.
        root.setFitsSystemWindows(!isTv);

        LinearLayout top = new LinearLayout(this);
        top.setGravity(Gravity.CENTER_VERTICAL);
        top.setPadding((int) (12 * dp), (int) (4 * dp), (int) (4 * dp), (int) (4 * dp));
        status = new TextView(this);
        status.setTextColor(Color.WHITE);
        status.setTextSize(14);
        status.setOnClickListener(v -> showCastHelp());
        top.addView(status, new LinearLayout.LayoutParams(0, LinearLayout.LayoutParams.WRAP_CONTENT, 1));
        top.addView(button("📺 Cast", v -> showCastHelp()));
        beTvButton = button("🖥 Be the TV", v -> askBeTv());
        top.addView(beTvButton);
        top.addView(button("⚙", v -> askServer(null)));
        bar = top;
        root.addView(bar);

        phone = new WebView(this);
        setUp(phone);
        phone.addJavascriptInterface(new Bridge(), "TriviaBixApp");
        phone.setWebViewClient(new Client() {
            @Override
            public void onReceivedError(WebView view, WebResourceRequest req, WebResourceError err) {
                if (req.isForMainFrame()) findServer(true);
            }

            @Override
            public void doUpdateVisitedHistory(WebView view, String url, boolean reload) {
                // The phone page's own "Use this device as the TV" lands here too.
                setTvPage(isTvPath(url));
            }
        });
        root.addView(phone, new LinearLayout.LayoutParams(LinearLayout.LayoutParams.MATCH_PARENT, 0, 1));
        setContentView(root);

        if (isTv) bar.setVisibility(View.GONE);
        for (String old : OLD_SERVERS) {
            if (old.equals(prefs.getString(KEY_SERVER, null))) prefs.edit().putString(KEY_SERVER, DEFAULT_SERVER).apply();
        }
        if (prefs.contains(KEY_SERVER)) openHome();
        else findServer(false);
        showStatus();
    }

    /** Fire TV sticks don't all report TV mode, so also look for their features. */
    private boolean isTvDevice() {
        UiModeManager ui = (UiModeManager) getSystemService(Context.UI_MODE_SERVICE);
        PackageManager pm = getPackageManager();
        return (ui != null && ui.getCurrentModeType() == Configuration.UI_MODE_TYPE_TELEVISION)
                || pm.hasSystemFeature(PackageManager.FEATURE_LEANBACK)
                || pm.hasSystemFeature("amazon.hardware.fire_tv");
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
    protected void onResume() {
        super.onResume();
        if (showingTvPage) fullScreen(true);
        phone.requestFocus();
    }

    @Override
    protected void onDestroy() {
        displays.unregisterDisplayListener(this);
        if (finder != null) finder.cancel();
        closeTv();
        phone.destroy();
        super.onDestroy();
    }

    @Override
    public void onWindowFocusChanged(boolean focused) {
        super.onWindowFocusChanged(focused);
        if (focused && showingTvPage) fullScreen(true);
    }

    @Override
    public void onBackPressed() {
        if (isTv) showTvMenu();
        else if (showingTvPage) phone.loadUrl(server() + "/");
        else if (phone.canGoBack()) phone.goBack();
        else super.onBackPressed();
    }

    /**
     * TV remotes: OK (and play/pause) act as Enter, which moves the TV page on and skips scenes;
     * the menu button opens the app's own menu.
     */
    @Override
    public boolean dispatchKeyEvent(KeyEvent e) {
        int code = e.getKeyCode();
        if (showingTvPage && (dialog == null || !dialog.isShowing())) {
            if (code == KeyEvent.KEYCODE_DPAD_CENTER || code == KeyEvent.KEYCODE_MEDIA_PLAY_PAUSE
                    || code == KeyEvent.KEYCODE_BUTTON_A) {
                return phone.dispatchKeyEvent(new KeyEvent(e.getDownTime(), e.getEventTime(), e.getAction(),
                        KeyEvent.KEYCODE_ENTER, e.getRepeatCount(), e.getMetaState()));
            }
            if (code == KeyEvent.KEYCODE_MENU && e.getAction() == KeyEvent.ACTION_UP) {
                if (isTv) showTvMenu();
                return true;
            }
        }
        return super.dispatchKeyEvent(e);
    }

    @Override public void onDisplayAdded(int id) { updateTv(); }
    @Override public void onDisplayRemoved(int id) { updateTv(); }
    @Override public void onDisplayChanged(int id) { updateTv(); }

    // ---------- What this screen shows ----------

    private void openHome() {
        phone.loadUrl(server() + (isTv ? "/tv" : "/"));
    }

    private static boolean isTvPath(String url) {
        String path = Uri.parse(url).getPath();
        return "/tv".equals(path) || "/host".equals(path);
    }

    /** The TV page goes sideways and full screen, with the app's bar out of the way. */
    private void setTvPage(boolean on) {
        if (on == showingTvPage) return;
        showingTvPage = on;
        if (!isTv) {
            bar.setVisibility(on ? View.GONE : View.VISIBLE);
            setRequestedOrientation(on
                    ? ActivityInfo.SCREEN_ORIENTATION_SENSOR_LANDSCAPE
                    : ActivityInfo.SCREEN_ORIENTATION_UNSPECIFIED);
            // A phone that is the TV is being mirrored, so a second TV page would open a second room.
            if (on) closeTv();
            else updateTv();
        }
        fullScreen(on);
    }

    @SuppressWarnings("deprecation")
    private void fullScreen(boolean on) {
        View decor = getWindow().getDecorView();
        decor.setSystemUiVisibility(on
                ? View.SYSTEM_UI_FLAG_IMMERSIVE_STICKY | View.SYSTEM_UI_FLAG_FULLSCREEN
                        | View.SYSTEM_UI_FLAG_HIDE_NAVIGATION | View.SYSTEM_UI_FLAG_LAYOUT_STABLE
                        | View.SYSTEM_UI_FLAG_LAYOUT_FULLSCREEN | View.SYSTEM_UI_FLAG_LAYOUT_HIDE_NAVIGATION
                : 0);
    }

    private void askBeTv() {
        new AlertDialog.Builder(this)
                .setTitle("Make this phone the TV?")
                .setMessage("Use this when Smart View only copies the phone screen onto the TV. This phone "
                        + "shows the game screen sideways and leaves the game, and everyone plays on their own "
                        + "phones. Mirror it to the TV with Smart View and keep it plugged in.\n\n"
                        + "Press back to come back to playing.")
                .setPositiveButton("Be the TV", (d, w) -> phone.loadUrl(server() + "/tv"))
                .setNegativeButton("Cancel", null)
                .show();
    }

    private void showTvMenu() {
        if (dialog != null && dialog.isShowing()) return;
        String[] items = { "Keep playing", "Reload the game screen", "Find the game server again",
                "Type the server address", "Close TriviaBix" };
        dialog = new AlertDialog.Builder(this)
                .setTitle("TriviaBix on " + server())
                .setItems(items, (d, which) -> {
                    if (which == 1) phone.reload();
                    else if (which == 2) findServer(false);
                    else if (which == 3) askServer(null);
                    else if (which == 4) finish();
                })
                .show();
    }

    // ---------- The second screen (Smart View) ----------

    /** Shows /tv on the first second screen, or takes it down when the cast ends. */
    private void updateTv() {
        Display[] screens = displays.getDisplays(DisplayManager.DISPLAY_CATEGORY_PRESENTATION);
        Display target = screens.length > 0 && !showingTvPage ? screens[0] : null;
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

    // ---------- The game server's address ----------

    private String server() {
        return prefs.getString(KEY_SERVER, DEFAULT_SERVER);
    }

    private void useServer(String url) {
        if (dialog != null) dialog.dismiss();
        prefs.edit().putString(KEY_SERVER, url).apply();
        openHome();
        closeTv();
        updateTv();
    }

    /** Looks for the game on this Wi-Fi, then offers what it found or asks for the address. */
    private void findServer(boolean failed) {
        if (finder != null || (dialog != null && dialog.isShowing())) return;
        dialog = new AlertDialog.Builder(this)
                .setTitle(failed ? "Can't reach the game" : "Finding the game")
                .setMessage((failed ? "Couldn't open " + server() + ". " : "")
                        + "Looking for TriviaBix on this Wi-Fi…")
                .setNegativeButton("Type the address", (d, w) -> { cancelFinder(); askServer(null); })
                .setCancelable(false)
                .show();
        finder = new ServerFinder(this, server(), found -> runOnUiThread(() -> {
            finder = null;
            if (isFinishing()) return;
            if (dialog != null) dialog.dismiss();
            dialog = null;
            if (found.isEmpty()) {
                askServer("Couldn't find TriviaBix on this Wi-Fi. Check this "
                        + (isTv ? "TV" : "phone") + " is on the home Wi-Fi and the server is running, "
                        + "or type the address you open TriviaBix at.");
            } else if (found.size() == 1 && !failed && !prefs.contains(KEY_SERVER)) {
                useServer(found.get(0).url);
            } else if (found.size() == 1 && found.get(0).url.equals(server())) {
                useServer(server()); // it's back
            } else {
                pickServer(found);
            }
        }));
        finder.start();
    }

    private void cancelFinder() {
        if (finder != null) finder.cancel();
        finder = null;
    }

    private void pickServer(List<ServerFinder.Found> found) {
        String[] items = new String[found.size() + 1];
        for (int i = 0; i < found.size(); i++) items[i] = found.get(i).label();
        items[found.size()] = "Type another address…";
        dialog = new AlertDialog.Builder(this)
                .setTitle("Pick the game server")
                .setItems(items, (d, which) -> {
                    if (which < found.size()) useServer(found.get(which).url);
                    else askServer(null);
                })
                .setCancelable(prefs.contains(KEY_SERVER))
                .show();
    }

    private void askServer(String problem) {
        if (dialog != null && dialog.isShowing()) dialog.dismiss();
        EditText input = new EditText(this);
        input.setInputType(InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_VARIATION_URI);
        input.setText(server());
        input.setSelectAllOnFocus(true);
        dialog = new AlertDialog.Builder(this)
                .setTitle(problem != null ? "Can't find the game" : "Game server address")
                .setMessage(problem != null ? problem
                        : "The address you open TriviaBix at, like " + DEFAULT_SERVER
                                + ". A Tailscale or other https address works too.")
                .setView(input)
                .setPositiveButton("Save", (d, w) -> {
                    String url = input.getText().toString().trim().replaceAll("/+$", "");
                    if (url.isEmpty()) url = DEFAULT_SERVER;
                    if (!url.matches("(?i)^https?://.*")) url = "http://" + url;
                    // Typed without a port: the server's own port, unless it's an https name.
                    if (url.matches("(?i)^http://[^/:]+$")) url = url + ":8080";
                    useServer(url);
                })
                .setNeutralButton("Find it", (d, w) -> findServer(false))
                .setCancelable(prefs.contains(KEY_SERVER))
                .show();
    }

    // ---------- Casting help ----------

    private void showCastHelp() {
        new AlertDialog.Builder(this)
                .setTitle("Put the game on the TV")
                .setMessage("1. Swipe down from the top of the screen twice and tap Smart View.\n"
                        + "2. Pick your TV. Choose to show the phone screen on the TV if it asks.\n"
                        + "3. Come back here. The TV shows the game, and this phone stays your controller.\n\n"
                        + "If the TV just copies this phone instead, tap 🖥 Be the TV and play on other "
                        + "phones, or put the TriviaBix app on a Fire TV stick, or open "
                        + server() + "/tv in the TV's browser.")
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
        s.setUserAgentString(s.getUserAgentString() + " TriviaBixCast/2");
        view.setBackgroundColor(BG);
        view.setFocusable(true);
        view.setFocusableInTouchMode(true);
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

        @JavascriptInterface
        public void beTv() {
            runOnUiThread(MainActivity.this::askBeTv);
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
