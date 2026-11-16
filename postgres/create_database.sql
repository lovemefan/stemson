-- Initialize an empty PostgreSQL database (create it before running this file).
-- The same seed credentials as MySQL: admin/admin123, lovemefan/lovemefan123.
-- Timestamps intentionally have no time zone, matching MySQL DATETIME and
-- the repository's NaiveDateTime. Connections use UTC.
BEGIN;

CREATE TABLE group_user (
    gid BIGINT NOT NULL CHECK (gid >= 0),
    uid BIGINT NOT NULL CHECK (uid >= 0),
    create_time TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (gid, uid)
);

-- Quoting is required: USER is a PostgreSQL keyword.
CREATE TABLE "user" (
    uid BIGINT PRIMARY KEY CHECK (uid >= 0),
    username VARCHAR(20) COLLATE "C" NOT NULL,
    phone VARCHAR(12),
    email VARCHAR(128),
    password VARCHAR(255),
    opaque_registration BYTEA,
    invite_token_hash CHAR(64),
    invite_expires_at TIMESTAMP,
    user_role INTEGER NOT NULL DEFAULT 0 CHECK (user_role BETWEEN 0 AND 255),
    create_time TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    create_by BIGINT NOT NULL CHECK (create_by >= 0),
    last_login_time TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
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
     1, NOW(), 1341983140255768576, '2020-12-24 13:37:38', 1, 1);

INSERT INTO "user"
    ("uid", "username", "phone", "email", "password", "user_role", "create_by", "identity")
VALUES
    (1342014866998829056, 'lovemefan', '', 'lovemefan@outlook.com',
     '$argon2id$v=19$m=19456,t=2,p=1$PrX26xKWFSn3gWo7/H/Y8Q$2HzFE6xqvoV5WHhMJ7+oPUHhmNMlXywl1j+VTT+fEWU',
     1, 1341983140255768576, 2);

INSERT INTO "group_user" (gid, uid) VALUES (1344262667581399040, 1341983140255768576);
INSERT INTO "group_user" (gid, uid) VALUES (1344262667581399040, 1342014866998829056);

COMMIT;
