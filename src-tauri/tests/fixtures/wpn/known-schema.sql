CREATE TABLE Notification (
    Id INTEGER PRIMARY KEY,
    HandlerId INTEGER NOT NULL,
    Payload BLOB NOT NULL,
    ArrivalTime
);

CREATE TABLE NotificationHandler (
    RecordId INTEGER PRIMARY KEY,
    PrimaryId TEXT,
    FixtureExtra TEXT
);

CREATE TABLE FixtureExtra (
    Id INTEGER PRIMARY KEY,
    Value TEXT
);
