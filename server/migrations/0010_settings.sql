-- Keys and addresses set from the admin panel's Settings page. A row here wins over the
-- container's environment variable of the same name; deleting it goes back to the variable.
CREATE TABLE settings (
    key        TEXT PRIMARY KEY,
    value      TEXT NOT NULL,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
