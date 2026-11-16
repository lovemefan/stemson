-- Initialize an empty SQLite file: sqlite3 stemson.db < sqlite/create_database.sql
-- Same seed accounts: admin/admin123, lovemefan/lovemefan123.
-- Timestamps are UTC text, decoded by SQLx as NaiveDateTime.
BEGIN;

CREATE TABLE group_user (
    gid INTEGER NOT NULL CHECK (gid >= 0),
    uid INTEGER NOT NULL CHECK (uid >= 0),
    create_time TEXT DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (gid, uid)
);

CREATE TABLE "user" (
    uid INTEGER PRIMARY KEY CHECK (uid >= 0),
    username TEXT COLLATE BINARY NOT NULL CHECK (length(username) <= 20),
    phone TEXT CHECK (length(phone) <= 12),
    email TEXT CHECK (length(email) <= 128),
    password TEXT CHECK (length(password) <= 255),
    opaque_registration BLOB,
    invite_token_hash TEXT CHECK (length(invite_token_hash) <= 64),
    invite_expires_at TEXT,
    user_role INTEGER NOT NULL DEFAULT 0 CHECK (user_role BETWEEN 0 AND 255),
    create_time TEXT DEFAULT CURRENT_TIMESTAMP,
    create_by INTEGER NOT NULL CHECK (create_by >= 0),
    last_login_time TEXT DEFAULT CURRENT_TIMESTAMP,
    status INTEGER DEFAULT 1 CHECK (status BETWEEN 0 AND 255),
    identity INTEGER DEFAULT 3 CHECK (identity BETWEEN 0 AND 255)
);
CREATE UNIQUE INDEX uk_username ON "user" (username);
CREATE INDEX pk_group_user_gid ON group_user (gid);
CREATE UNIQUE INDEX pk_group_user_uid ON group_user (uid);

INSERT INTO "user"
    ("uid", "username", "phone", "email", "password", "user_role", "create_time", "create_by", "last_login_time", "status", "identity")
VALUES
    (1341983140255768576, 'admin', NULL, NULL,
     '$argon2id$v=19$m=19456,t=2,p=1$0gRpv9+iRxe8Iloo3FC32Q$SIRoE/uS8AuTRhndkIYQhFCPDTrVh30LB3ilEv7DHEI',
     1, CURRENT_TIMESTAMP, 1341983140255768576, '2020-12-24 13:37:38', 1, 1);

INSERT INTO "user"
    ("uid", "username", "phone", "email", "password", "user_role", "create_by", "identity")
VALUES
    (1342014866998829056, 'lovemefan', '', 'lovemefan@outlook.com',
     '$argon2id$v=19$m=19456,t=2,p=1$PrX26xKWFSn3gWo7/H/Y8Q$2HzFE6xqvoV5WHhMJ7+oPUHhmNMlXywl1j+VTT+fEWU',
     1, 1341983140255768576, 2);

INSERT INTO "group_user" (gid, uid) VALUES (1344262667581399040, 1341983140255768576);
INSERT INTO "group_user" (gid, uid) VALUES (1344262667581399040, 1342014866998829056);

COMMIT;
