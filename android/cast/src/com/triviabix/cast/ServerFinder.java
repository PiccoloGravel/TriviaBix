package com.triviabix.cast;

import android.content.Context;
import android.net.ConnectivityManager;
import android.net.LinkAddress;
import android.net.LinkProperties;
import android.net.Network;
import android.net.Uri;

import java.io.ByteArrayOutputStream;
import java.io.InputStream;
import java.net.HttpURLConnection;
import java.net.Inet4Address;
import java.net.InetAddress;
import java.net.URL;
import java.util.ArrayList;
import java.util.Collections;
import java.util.LinkedHashSet;
import java.util.List;
import java.util.Set;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.TimeUnit;
import java.util.regex.Matcher;
import java.util.regex.Pattern;

/**
 * Finds TriviaBix servers on this Wi-Fi without anyone typing an address: it asks every address
 * on the local network (the /24 around this device) for /healthz on the game's usual ports, and
 * keeps the ones that answer like TriviaBix. The saved address and the public address are tried
 * too, so an https address still counts when it answers.
 */
final class ServerFinder {
    interface Done { void found(List<Found> servers); }

    static final class Found {
        final String url;
        final int questions;

        Found(String url, int questions) {
            this.url = url;
            this.questions = questions;
        }

        String label() {
            return url.replaceFirst("^https?://", "") + (questions > 0 ? "   (" + questions + " questions)" : "");
        }
    }

    /** The server's own port, and the one the Unraid install publishes it on. */
    private static final int[] PORTS = { 8080, 8090 };
    private static final Pattern QUESTIONS = Pattern.compile("\"questions\"\\s*:\\s*(\\d+)");

    private final Context context;
    private final String saved;
    private final Done done;
    private volatile boolean cancelled;
    private Thread thread;

    ServerFinder(Context context, String saved, Done done) {
        this.context = context.getApplicationContext();
        this.saved = saved;
        this.done = done;
    }

    void start() {
        thread = new Thread(this::run, "find-triviabix");
        thread.start();
    }

    void cancel() {
        cancelled = true;
    }

    private void run() {
        Set<String> candidates = new LinkedHashSet<>();
        candidates.add(saved);
        candidates.add(MainActivity.DEFAULT_SERVER);
        Set<Integer> ports = new LinkedHashSet<>();
        for (int p : PORTS) ports.add(p);
        int savedPort = Uri.parse(saved).getPort();
        if (savedPort > 0 && saved.startsWith("http://")) ports.add(savedPort);
        for (String prefix : localPrefixes()) {
            for (int i = 1; i < 255; i++) {
                for (int port : ports) candidates.add("http://" + prefix + i + ":" + port);
            }
        }
        List<Found> found = Collections.synchronizedList(new ArrayList<>());
        ExecutorService pool = Executors.newFixedThreadPool(48);
        for (String url : candidates) {
            pool.execute(() -> {
                if (cancelled) return;
                boolean named = url.equals(saved) || url.equals(MainActivity.DEFAULT_SERVER);
                int q = probe(url, named ? 3000 : 700);
                if (q >= 0) found.add(new Found(url, q));
            });
        }
        pool.shutdown();
        try {
            pool.awaitTermination(30, TimeUnit.SECONDS);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
        }
        pool.shutdownNow();
        if (cancelled) return;
        List<Found> sorted;
        synchronized (found) {
            sorted = new ArrayList<>(found);
        }
        // The saved address first, then the public one, then by address.
        sorted.sort((a, b) -> Integer.compare(rank(a.url), rank(b.url)) != 0
                ? Integer.compare(rank(a.url), rank(b.url)) : a.url.compareTo(b.url));
        done.found(sorted);
    }

    private int rank(String url) {
        return url.equals(saved) ? 0 : url.equals(MainActivity.DEFAULT_SERVER) ? 1 : 2;
    }

    /** "10.0.0." for each IPv4 network this device is on (Wi-Fi or Ethernet on a TV stick). */
    private List<String> localPrefixes() {
        List<String> out = new ArrayList<>();
        ConnectivityManager cm = (ConnectivityManager) context.getSystemService(Context.CONNECTIVITY_SERVICE);
        if (cm == null) return out;
        Network net = cm.getActiveNetwork();
        LinkProperties props = net == null ? null : cm.getLinkProperties(net);
        if (props == null) return out;
        for (LinkAddress la : props.getLinkAddresses()) {
            InetAddress a = la.getAddress();
            if (!(a instanceof Inet4Address) || a.isLoopbackAddress()) continue;
            byte[] b = a.getAddress();
            out.add((b[0] & 255) + "." + (b[1] & 255) + "." + (b[2] & 255) + ".");
        }
        return out;
    }

    /** The server's question count, or -1 if nothing like TriviaBix answers there. */
    static int probe(String base, int timeoutMs) {
        HttpURLConnection c = null;
        try {
            c = (HttpURLConnection) new URL(base + "/healthz").openConnection();
            c.setConnectTimeout(timeoutMs);
            c.setReadTimeout(timeoutMs * 2);
            c.setInstanceFollowRedirects(false);
            if (c.getResponseCode() != 200) return -1;
            ByteArrayOutputStream body = new ByteArrayOutputStream();
            try (InputStream in = c.getInputStream()) {
                byte[] buf = new byte[1024];
                int n;
                while ((n = in.read(buf)) > 0 && body.size() < 4096) body.write(buf, 0, n);
            }
            Matcher m = QUESTIONS.matcher(body.toString("UTF-8"));
            if (m.find()) return Integer.parseInt(m.group(1));
            return body.toString("UTF-8").contains("\"triviabix\"") ? 0 : -1;
        } catch (Exception e) {
            return -1;
        } finally {
            if (c != null) c.disconnect();
        }
    }
}
