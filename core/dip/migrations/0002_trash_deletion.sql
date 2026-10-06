-- The kind 5 that put an event in the trash, NULL when the user did. A later
-- kind 5 retracting that one takes back out exactly what it put in, and leaves
-- what the user threw out themselves (docs/storage.md#the-trash).
ALTER TABLE event_trashed ADD COLUMN deletion TEXT;

CREATE INDEX event_trashed_deletion ON event_trashed (deletion) WHERE deletion IS NOT NULL;
