CREATE TABLE Notification (
    Id INTEGER PRIMARY KEY,
    HandlerId INTEGER NOT NULL,
    RenamedPayload BLOB NOT NULL,
    ArrivalTime
);

CREATE TABLE NotificationHandler (
    RecordId INTEGER PRIMARY KEY,
    PrimaryId TEXT
);
