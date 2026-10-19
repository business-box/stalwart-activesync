-- Gateway bookkeeping only; losing it costs each device one full resync.

CREATE TABLE devices (
    id                 INTEGER PRIMARY KEY,
    username           TEXT    NOT NULL,
    device_id          TEXT    NOT NULL,
    device_type        TEXT    NOT NULL,
    user_agent         TEXT,
    protocol_version   TEXT,
    -- Policy key the device acknowledged; NULL until provisioned.
    policy_key         INTEGER,
    -- Temporary key handed out in the first Provision round trip.
    pending_policy_key INTEGER,
    -- 0 none, 1 wipe requested, 2 wipe acknowledged by the device.
    wipe_status        INTEGER NOT NULL DEFAULT 0,
    blocked            INTEGER NOT NULL DEFAULT 0,
    model              TEXT,
    friendly_name      TEXT,
    os                 TEXT,
    -- Last full Sync request, replayed when the device sends an empty one.
    last_sync_request  BLOB,
    first_seen         INTEGER NOT NULL,
    last_seen          INTEGER NOT NULL,
    UNIQUE (username, device_id)
);

-- What the device knows of the folder tree, and the stable short IDs.
CREATE TABLE folders (
    device       INTEGER NOT NULL REFERENCES devices (id) ON DELETE CASCADE,
    server_id    TEXT    NOT NULL,
    kind         TEXT    NOT NULL,
    jmap_id      TEXT    NOT NULL,
    folder_type  INTEGER NOT NULL,
    parent_id    TEXT    NOT NULL,
    display_name TEXT    NOT NULL,
    PRIMARY KEY (device, server_id),
    UNIQUE (device, kind, jmap_id)
);

CREATE TABLE hierarchy_state (
    device         INTEGER PRIMARY KEY REFERENCES devices (id) ON DELETE CASCADE,
    sync_key       TEXT    NOT NULL,
    prev_key       TEXT,
    prev_response  BLOB,
    counter        INTEGER NOT NULL,
    next_folder_id INTEGER NOT NULL
);

CREATE TABLE sync_state (
    device        INTEGER NOT NULL REFERENCES devices (id) ON DELETE CASCADE,
    folder        TEXT    NOT NULL,
    sync_key      TEXT    NOT NULL,
    prev_key      TEXT,
    -- Encoded Collection element answering prev_key, for retries.
    prev_response BLOB,
    counter       INTEGER NOT NULL,
    jmap_state    TEXT,
    filter_type   INTEGER NOT NULL DEFAULT 0,
    -- Encoded Options element; devices omit Options once they sent them.
    options       BLOB,
    PRIMARY KEY (device, folder)
);

CREATE TABLE items (
    device    INTEGER NOT NULL REFERENCES devices (id) ON DELETE CASCADE,
    folder    TEXT    NOT NULL,
    server_id TEXT    NOT NULL,
    jmap_id   TEXT    NOT NULL,
    hash      TEXT    NOT NULL,
    PRIMARY KEY (device, folder, server_id)
);

CREATE TABLE client_ids (
    device    INTEGER NOT NULL REFERENCES devices (id) ON DELETE CASCADE,
    client_id TEXT    NOT NULL,
    jmap_id   TEXT    NOT NULL,
    PRIMARY KEY (device, client_id)
);

CREATE TABLE ping_state (
    device    INTEGER PRIMARY KEY REFERENCES devices (id) ON DELETE CASCADE,
    heartbeat INTEGER NOT NULL,
    folders   TEXT    NOT NULL
);
