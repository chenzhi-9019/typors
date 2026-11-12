# document-session Specification Delta

## Purpose

Defines per-file reading-position memory: typors records where the user was in each document when it closes and returns to that place when the same file is opened again, without changing the file or the document's dirty state.

## ADDED Requirements

### Requirement: Reading position is recorded on close

When a document with a file path loses its last window or the application exits normally, typors SHALL record the caret byte offset and the vertical scroll offset for that document's canonical absolute path. The record SHALL also store the file identity used for validation: size and content hash at record time.

#### Scenario: Close records the position

- **WHEN** the user scrolls to the middle of a file, places the caret in a paragraph, and closes the window
- **THEN** a record for that absolute path exists with the caret offset, scroll offset, file size and content hash

#### Scenario: Exit with several windows

- **WHEN** several documents are open and the application exits
- **THEN** every open document with a path has its own record

### Requirement: Position is restored on reopen

When a file with a matching record is opened and the file's size and content hash still match the record, typors SHALL restore the caret to the recorded byte offset and the scroll to the recorded offset before the first painted frame, with the caret visible.

#### Scenario: Reopen returns to the same place

- **WHEN** a previously closed file is opened again unchanged
- **THEN** the first painted frame shows the recorded scroll position with the caret at the recorded offset

#### Scenario: Restore respects page width

- **WHEN** a position recorded at one window width is restored at another width
- **THEN** the vertical scroll offset is applied, the caret is scrolled into view, and the caret remains the anchor

### Requirement: Changed files are clamped, not guessed

When the file changed since the record was written, typors SHALL NOT restore the raw offsets blindly. The caret SHALL be clamped to a valid character boundary inside the new content, the scroll offset SHALL be clamped to the new document extent, and the recorded identity SHALL be replaced on the next record.

#### Scenario: File grew

- **WHEN** the recorded caret offset is still inside the new content
- **THEN** the caret is restored at that offset and the scroll is clamped to the new extent

#### Scenario: File shrank past the offset

- **WHEN** the recorded offset is beyond the new file length
- **THEN** the caret is placed at the nearest valid boundary at or before the recorded offset, and no error is shown

### Requirement: Scope and exclusions

Records SHALL be keyed by canonical absolute path, SHALL cover only documents that have a path, and SHALL be independent of the on-disk render cache. Untitled documents, probe-driven headless runs and documents whose path cannot be canonicalised SHALL NOT create records.

#### Scenario: Untitled document

- **WHEN** the user types in an unsaved document and closes it
- **THEN** no reading-position record is created

#### Scenario: Cache cleared, position kept

- **WHEN** the user clears the render cache and reopens a file
- **THEN** the reading position is still restored

### Requirement: Restore never edits the document

Restoring a reading position SHALL NOT modify the document content, SHALL NOT mark the document dirty, SHALL NOT push an undo step, and SHALL NOT write the opened file.

#### Scenario: Clean document stays clean

- **WHEN** a file is reopened and the position is restored
- **THEN** the title has no modified marker, undo is empty, and the file's bytes are unchanged

### Requirement: Checkpointing limits data loss

For documents open for a long time, typors SHALL also write the position periodically and when the window loses focus, so an abnormal termination loses at most the configured checkpoint interval of movement. Checkpoints SHALL NOT be more frequent than the documented interval and SHALL NOT block typing.

#### Scenario: Crash before close

- **WHEN** typors terminates abnormally after the user scrolled and stayed idle past one checkpoint interval
- **THEN** the next open restores the position captured by the last checkpoint

### Requirement: Persistence is failure-tolerant

A missing, unreadable, corrupt or unwritable position store SHALL degrade to "no position recorded": opening still succeeds, renders from the top, and the failure never blocks or fails a save, close or open. Writes SHALL be atomic.

#### Scenario: Corrupt store

- **WHEN** the position store is corrupt JSON
- **THEN** typors ignores it, opens the document at the top, and rewrites a valid store on the next record

#### Scenario: Read-only profile

- **WHEN** the position store cannot be written
- **THEN** open, edit, save and close all succeed and no dialog interrupts the user

### Requirement: Stale records are cleaned up

Records for files that no longer exist SHALL be removed after the documented retention period, and the store SHALL be bounded to a documented maximum number of entries by evicting the least recently used.

#### Scenario: Deleted file

- **WHEN** a recorded file has been deleted and the retention period has passed
- **THEN** its record is removed and the store size stays bounded

### Requirement: Position behaviour is observable

A diagnostic mode SHALL report the store path, the number of records, and for one document the recorded and effective caret and scroll offsets after a simulated close and reopen.

#### Scenario: Probe round-trip

- **WHEN** the position probe records a position, reloads the store, and reopens the document
- **THEN** it prints the recorded and effective offsets, asserts they match, and exits 0
