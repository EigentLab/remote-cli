-- A User's Address Book in the Dashboard: Devices added by Device ID and Device Password (ADR 0011).
-- `password` is sealed with RCLI_MASTER_KEY (aad: user id || device id) when one is set, else plaintext.
CREATE TABLE address_book (
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    device UUID NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    password BYTEA NOT NULL,
    password_sealed BOOLEAN NOT NULL,
    added_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (user_id, device)
);
