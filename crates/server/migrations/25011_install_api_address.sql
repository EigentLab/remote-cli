-- The API Address the install's mode gave (`RCLI_API_ADDRESS`) at the last start: a start applies it only when it changed.
ALTER TABLE server_ca ADD COLUMN install_api_address TEXT NOT NULL DEFAULT '';
