-- Wake (ADR 0011): an Address Book entry keeps the Device's MAC and subnet from its last connection.
ALTER TABLE address_book ADD COLUMN mac TEXT, ADD COLUMN subnet TEXT;
