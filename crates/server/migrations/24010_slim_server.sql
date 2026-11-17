-- The Server only connects (ADR 0011): no resource history, stored inventory, audit log or Device management.
DROP TABLE device_inventory, resource_samples, resource_rollups, audit_log;
ALTER TABLE devices
    DROP COLUMN name,
    DROP COLUMN enrolled_at,
    DROP COLUMN wake_requested_at,
    DROP COLUMN revoked_at,
    DROP COLUMN update_error;
