-- Rendering jobs started from the admin panel (server/src/render.rs): voice and art on a rented
-- vast.ai GPU, and host lines from Claude. Times are milliseconds since the epoch; cost is in
-- dollars of GPU time. `instance` is the vast.ai instance while one is rented, and `released`
-- says it was destroyed, so a restart can destroy one a job left behind.
CREATE TABLE render_jobs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    kind TEXT NOT NULL,
    options TEXT NOT NULL DEFAULT '{}',
    state TEXT NOT NULL,
    started_at INTEGER NOT NULL,
    finished_at INTEGER,
    exit_code INTEGER,
    gpu TEXT,
    price_hr REAL,
    max_cost REAL,
    cost REAL,
    instance INTEGER,
    released BOOLEAN NOT NULL DEFAULT 1,
    details TEXT,
    log TEXT NOT NULL DEFAULT ''
);
