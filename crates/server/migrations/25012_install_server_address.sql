-- The Server Address the install gave (`RCLI_AGENT_HOSTNAMES`) at the last start: a start applies it only when it
-- changed. NULL on a Server from before: its first start records it without replacing the Users' address.
ALTER TABLE server_ca ADD COLUMN install_server_address TEXT;
