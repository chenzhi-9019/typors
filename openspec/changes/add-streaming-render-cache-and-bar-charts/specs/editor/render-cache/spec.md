# render-cache Specification Delta

## Purpose

Defines the persistent, on-disk cache of rendered resources: where it lives, how a document's absolute path maps to its cache directory, what is cached and how it is validated, and how the cache stays bounded and safe.

## ADDED Requirements

### Requirement: Cache root location

The cache root SHALL be `<directory containing typors.exe>\cache`. When that directory does not exist and cannot be created, or exists but is not writable, typors SHALL fall back to `%LOCALAPPDATA%\typors\cache`, and SHALL use the first writable root for the whole session.

#### Scenario: Portable install writes beside the executable

- **WHEN** typors runs from a writable directory such as a USB stick or a user folder
- **THEN** the cache is created and used in `<exe dir>\cache`

#### Scenario: Program Files install falls back

- **WHEN** typors is installed under `Program Files` and the executable directory is not writable for the current user
- **THEN** typors uses `%LOCALAPPDATA%\typors\cache` and the editor still opens, renders and caches

### Requirement: Absolute path to cache directory mapping

Each document SHALL own exactly one cache directory, derived deterministically from the document's canonical absolute path, and typors SHALL maintain an index that maps absolute path to cache directory name. The mapping SHALL be stable across launches and SHALL distinguish paths that differ only by case, drive or unc path.

#### Scenario: Same file reopens into the same directory

- **WHEN** the same file is opened in two separate sessions
- **THEN** both sessions use the same cache directory and the second session reports cache hits

#### Scenario: Two files with the same name

- **WHEN** `C:\a\notes.md` and `C:\b\notes.md` are both opened
- **THEN** each has its own cache directory and neither reads the other's resources

#### Scenario: File moved or renamed

- **WHEN** a cached file is renamed and reopened
- **THEN** a new cache directory is created for the new absolute path, and the old entry is reclaimed by eviction rather than reused

### Requirement: Cached resource families

The cache SHALL store, per document: decoded image bitmaps for local, remote and data sources; rasterised chart and Mermaid images; math layout output; and per-block layout geometry. An entry SHALL be reusable across processes and SHALL NOT require re-running the producer when a valid entry exists.

#### Scenario: Images are not decoded twice

- **WHEN** a document containing local images is reopened in a new process
- **THEN** those images are painted from cache without re-reading and re-decoding the source files

#### Scenario: Chart and Mermaid rasters are reused

- **WHEN** a document containing `chart` and `mermaid` fences is reopened at the same size and scale
- **THEN** the rasterised images are loaded from cache and the SVG pipeline is not re-run

### Requirement: Content-based validation

A cache entry SHALL be valid only while its key still matches: document identity (absolute path, size and content hash), and for each resource the resource's own inputs (source bytes or chart/Mermaid text, pixel size, scale factor, font, theme and DPI). A mismatch SHALL invalidate only the affected entries.

#### Scenario: Edited document

- **WHEN** a document is edited and saved, then reopened
- **THEN** entries whose inputs changed are recomputed and unchanged entries are still reused

#### Scenario: Theme or DPI change

- **WHEN** the user switches between light and dark theme, or moves the window to a monitor with a different scale factor
- **THEN** resources whose appearance depends on it are recomputed and no stale-colour entry is painted

#### Scenario: File timestamp without content change

- **WHEN** a file's modification time changes but its bytes are identical
- **THEN** its cached entries remain valid

### Requirement: Bounded cache size with eviction

The cache SHALL be bounded by a documented size limit with a documented default, SHALL evict least-recently-used document directories when the limit is exceeded, and SHALL NOT evict entries belonging to documents currently open. Eviction SHALL run without blocking the first paint of an open document.

#### Scenario: Limit is respected

- **WHEN** cached data exceeds the configured limit after new writes
- **THEN** least-recently-used document directories are removed until usage is under the limit

#### Scenario: Open document is not evicted

- **WHEN** eviction selects candidates while a document is open
- **THEN** that document's cache directory is kept and its resources remain usable

### Requirement: Cache safety and corruption tolerance

Cache writes SHALL be atomic so a crash or a concurrent typors instance cannot leave a half-written entry that is later trusted. A corrupt, truncated or unreadable entry SHALL be treated as a miss, ignored and overwritten, and SHALL never cause a crash or a blank render. A cache root that becomes unavailable at runtime SHALL degrade to in-memory caching.

#### Scenario: Corrupt entry

- **WHEN** a cached image or geometry file is truncated or has garbage bytes
- **THEN** typors ignores it, re-renders the resource, and rewrites the entry

#### Scenario: Two instances at once

- **WHEN** two typors processes render the same document concurrently
- **THEN** both succeed, and the cache contains one valid entry per resource rather than a mixed or partial file

#### Scenario: Cache directory deleted while running

- **WHEN** the user deletes the cache directory while typors is running
- **THEN** rendering continues from memory, and the next run recreates the cache

### Requirement: Cache does not change output

With a valid cache, a frame SHALL paint the same geometry, colours and text as with an empty cache. Cached results SHALL NOT be used when the inputs that produced them cannot be verified.

#### Scenario: Cold and warm renders match

- **WHEN** the same document is rendered once with an empty cache and once with a warm cache
- **THEN** geometry fingerprints and painted content are identical

### Requirement: Cache management

typors SHALL provide a documented way to clear the cache and to report its location, size and per-document entries. Clearing the cache SHALL be safe while typors is running and SHALL NOT alter any document.

#### Scenario: Clear cache from the command line

- **WHEN** the user runs the documented clear-cache invocation
- **THEN** cached data is removed, the next open renders from scratch, and documents are unchanged

#### Scenario: Report cache usage

- **WHEN** the user asks for cache status
- **THEN** typors prints the active cache root, total size and the number of document entries
