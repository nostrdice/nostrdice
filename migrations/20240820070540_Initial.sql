CREATE TABLE IF NOT EXISTS zaps (
    payment_hash TEXT NOT NULL PRIMARY KEY,
    roller TEXT NOT NULL,
    invoice TEXT NOT NULL,
    request_event TEXT NOT NULL,
    multiplier_note_id TEXT NOT NULL,
    nonce_commitment_note_id TEXT NOT NULL,
    bet_state TEXT NOT NULL,
    idx INTEGER NOT NULL,
    bet_timestamp datetime NOT NULL
);

CREATE TABLE IF NOT EXISTS nonces (
    event_id TEXT NOT NULL PRIMARY KEY,
    nonce TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS active_nonce (
    id INTEGER NOT NULL PRIMARY KEY CHECK (id = 0),
    nonce_event_id TEXT NOT NULL REFERENCES nonces(event_id)
);

CREATE TABLE IF NOT EXISTS latest_expired_nonce (
    id INTEGER NOT NULL PRIMARY KEY CHECK (id = 0),
    nonce_event_id TEXT NOT NULL REFERENCES nonces(event_id)
);
