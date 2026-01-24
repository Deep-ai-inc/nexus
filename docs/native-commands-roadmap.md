# Native Commands Roadmap

This document lists native commands that can be implemented to leverage the structured `Value` pipeline system.

## Current Status

**Implemented:**
- `ls` - List directory contents (returns `List<FileEntry>` or `Table`)
- `head` - First N items from list/table/string

---

## Tier 1: Dead Simple (< 50 lines each)

| Command | Stdin Type | Output | Notes |
|---------|-----------|--------|-------|
| `tail -n N` | List/Table/String | Same type, last N | Mirror of `head` |
| `wc -l` | List/Table/String | Int (count) | Just `.len()` |
| `wc -w` | String | Int | Split and count |
| `wc -c` | String/Bytes | Int | Byte count |
| `cat` | Any | Same | Pass-through (for files: read and emit) |
| `echo` | - | String | Trivial |
| `pwd` | - | Path | Return `ctx.state.cwd` |
| `true` | - | Unit | Exit 0 |
| `false` | - | Unit | Exit 1 |
| `rev` | List/String | Same | Reverse items/chars |
| `tac` | List | List | Reverse lines |
| `uniq` | List | List | Dedupe adjacent |

---

## Tier 2: Moderate (50-150 lines)

| Command | Stdin Type | Output | Notes |
|---------|-----------|--------|-------|
| `sort` | List<FileEntry> | List | Sort by name |
| `sort -S` | List<FileEntry> | List | Sort by size |
| `sort -t` | List<FileEntry> | List | Sort by mtime |
| `sort -r` | List | List | Reverse sort |
| `grep PATTERN` | List<FileEntry> | List | Filter by name match |
| `grep PATTERN` | List<String> | List | Filter matching lines |
| `grep -v` | Any | Same | Invert match |
| `grep -i` | Any | Same | Case insensitive |
| `cut -d, -f1` | List<String> | List | Extract fields |
| `tr a-z A-Z` | String | String | Character translation |
| `basename` | Path/FileEntry | String | Extract filename |
| `dirname` | Path/FileEntry | Path | Extract directory |
| `realpath` | Path | Path | Canonicalize |
| `seq N` | - | List<Int> | Generate 1..N |
| `seq N M` | - | List<Int> | Generate N..M |
| `env` | - | Table | List env vars |
| `printenv VAR` | - | String | Get env var |

---

## Tier 3: Useful but More Complex (150-300 lines)

| Command | Notes |
|---------|-------|
| `find PATH -name PATTERN` | Walk directory, filter by glob |
| `find PATH -type f/d` | Filter by file type |
| `stat FILE` | Return FileEntry with full metadata |
| `du -h` | Directory size (recursive) |
| `touch FILE` | Create/update timestamp |
| `mkdir [-p]` | Create directory |
| `rm [-r]` | Remove file/directory |
| `cp SRC DST` | Copy file |
| `mv SRC DST` | Move/rename |
| `ln -s` | Create symlink |
| `chmod MODE` | Change permissions |
| `xargs CMD` | Run command for each stdin item |
| `tee FILE` | Write to file and pass through |
| `diff FILE1 FILE2` | Compare files |

---

## String/Text Processing

| Command | Stdin | Output | Implementation |
|---------|-------|--------|----------------|
| `nl` | List<String> | List<String> | Prepend line numbers |
| `fold -w N` | String | String | Wrap at N chars |
| `fmt -w N` | String | String | Reflow paragraphs |
| `expand` | String | String | Tabs → spaces |
| `unexpand` | String | String | Spaces → tabs |
| `paste -d,` | List | String | Join with delimiter |
| `join` | Two Lists | List | SQL-like join |
| `split -l N` | List | List<List> | Chunk into groups |
| `shuf` | List | List | Randomize order |
| `head -c N` | String/Bytes | Same | First N bytes |
| `tail -c N` | String/Bytes | Same | Last N bytes |
| `strings` | Bytes | List<String> | Extract printable sequences |
| `od -c` | Bytes | String | Octal/hex dump |
| `xxd` | Bytes | String | Hex dump |
| `base64` | Bytes | String | Encode |
| `base64 -d` | String | Bytes | Decode |

---

## Filtering & Selection

| Command | Stdin | Output | Implementation |
|---------|-------|--------|----------------|
| `grep -c` | List | Int | Count matches |
| `grep -l` | List<FileEntry> | List | Files containing match |
| `grep -n` | List<String> | List | Include line numbers |
| `grep -o` | String | List<String> | Only matching parts |
| `grep -E` | Any | Same | Extended regex |
| `grep -F` | Any | Same | Fixed string (fast) |
| `awk '{print $1}'` | List<String> | List | Field extraction |
| `awk '/pat/'` | List | List | Pattern filter |
| `sed 's/a/b/'` | String/List | Same | Substitute |
| `sed '/pat/d'` | List | List | Delete matching |
| `uniq -c` | List | List<(count, item)> | Count occurrences |
| `uniq -d` | List | List | Only duplicates |
| `uniq -u` | List | List | Only unique |
| `comm` | Two Lists | Table | Compare sorted lists |
| `yes [STRING]` | - | Stream<String> | Infinite repeater |
| `repeat N CMD` | - | List | Run N times |

---

## Math & Aggregation

| Command | Stdin | Output | Implementation |
|---------|-------|--------|----------------|
| `sum` | List<Int/Float> | Number | Add all |
| `avg` | List<Int/Float> | Float | Average |
| `min` | List<Number> | Number | Minimum |
| `max` | List<Number> | Number | Maximum |
| `count` | List | Int | Same as `wc -l` |
| `expr` | - | Int | `expr 1 + 2` |
| `bc` | String | String | Calculator |
| `factor N` | - | List<Int> | Prime factors |
| `seq -s,` | - | String | With separator |
| `seq -w` | - | List | Zero-padded |
| `jot N MIN MAX` | - | List | BSD-style seq |

---

## File Metadata & Inspection

| Command | Stdin | Output | Implementation |
|---------|-------|--------|----------------|
| `file FILE` | Path/FileEntry | String | Detect file type |
| `stat -f %z` | FileEntry | Int | Just size |
| `stat -f %m` | FileEntry | Int | Just mtime |
| `readlink` | Path | Path | Resolve symlink |
| `namei` | Path | Table | Path component breakdown |
| `test -f/-d/-e` | Path | Bool | File tests |
| `[ -f FILE ]` | Path | Bool | Same as test |
| `ls -S` | Path | List<FileEntry> | Sorted by size |
| `ls -t` | Path | List<FileEntry> | Sorted by time |
| `ls -r` | Path | List<FileEntry> | Reversed |
| `ls -R` | Path | List<FileEntry> | Recursive |
| `tree` | Path | Tree<FileEntry> | Directory tree |
| `exa` | Path | List<FileEntry> | Modern ls (alias) |

---

## Selection & Slicing

| Command | Stdin | Output | Implementation |
|---------|-------|--------|----------------|
| `first` | List | Item | First element |
| `last` | List | Item | Last element |
| `nth N` | List | Item | Nth element |
| `take N` | List | List | First N (alias head) |
| `skip N` | List | List | Skip first N |
| `slice N M` | List | List | Elements N to M |
| `every N` | List | List | Every Nth element |
| `where COND` | List | List | Filter by condition |
| `select COLS` | Table | Table | Select columns |
| `reject COLS` | Table | Table | Remove columns |
| `flatten` | List<List> | List | Flatten one level |
| `transpose` | Table | Table | Swap rows/cols |
| `zip` | Two Lists | List<Tuple> | Pair up elements |

---

## Type Conversion

| Command | Stdin | Output | Implementation |
|---------|-------|--------|----------------|
| `lines` | String | List<String> | Split by newline |
| `words` | String | List<String> | Split by whitespace |
| `chars` | String | List<String> | Split into chars |
| `bytes` | String | List<Int> | UTF-8 byte values |
| `from-json` | String | Value | Parse JSON |
| `to-json` | Value | String | Serialize JSON |
| `from-csv` | String | Table | Parse CSV |
| `to-csv` | Table | String | Serialize CSV |
| `to-text` | Value | String | Convert to plain text |
| `to-table` | List<Record> | Table | Restructure |
| `to-list` | Table | List<Record> | Restructure |
| `collect` | Stream | List | Gather all items |

---

## Path Manipulation

| Command | Stdin | Output | Implementation |
|---------|-------|--------|----------------|
| `basename -s .rs` | Path | String | Remove suffix too |
| `dirname -z` | List<Path> | List<Path> | Batch mode |
| `extname` | Path | String | Get extension |
| `stem` | Path | String | Filename without ext |
| `parent` | Path | Path | Parent directory |
| `join-path` | List<String> | Path | Combine path parts |
| `split-path` | Path | List<String> | Break into parts |
| `is-absolute` | Path | Bool | Check if absolute |
| `normalize` | Path | Path | Clean up `..` and `.` |

---

## Environment & Shell

| Command | Stdin | Output | Implementation |
|---------|-------|--------|----------------|
| `which CMD` | - | Path | Find executable |
| `type CMD` | - | String | Describe command |
| `alias` | - | Table | List aliases |
| `history` | - | List<String> | Command history |
| `whoami` | - | String | Current user |
| `hostname` | - | String | Machine name |
| `uname -a` | - | Record | System info |
| `date` | - | String | Current time |
| `date +%s` | - | Int | Unix timestamp |
| `sleep N` | - | Unit | Pause N seconds |
| `time CMD` | - | Record | Measure duration |

---

## Example Pipelines

### Basic (already possible)
```bash
ls | head -5
```

### With Tier 1 commands
```bash
ls | tail -5
ls | wc -l
ls | rev
```

### With Tier 2 commands
```bash
ls | sort -S | head -10          # Top 10 largest files
ls | grep "\.rs$"                # Rust files only
ls | sort -t | tail -5           # 5 most recently modified
ls -la | cut -f1,4               # Just permissions and name
env | grep PATH                  # Find PATH vars
seq 100 | head -10               # First 10 numbers
```

### With Tier 3 commands
```bash
find . -name "*.rs" | wc -l      # Count Rust files
find . -type f | sort -S | head  # Largest files anywhere
ls | xargs stat                  # Full metadata for all
```

### Advanced pipelines
```bash
# Stats
ls | wc -l                           # File count
ls | sort -S | last                  # Largest file
ls | avg size                        # Average file size

# Text processing
cat file | lines | shuf | head -1    # Random line
cat file | words | uniq | count      # Unique word count
cat file | lines | nl                # Number lines

# Data transformation
env | to-json                        # Env as JSON
ls | to-csv > files.csv              # Export file list
cat data.json | from-json | select name,age

# Path manipulation
ls | basename | extname | uniq -c    # Extensions histogram
find . -name "*.rs" | dirname | uniq # Directories with Rust

# Chained filters
ls | grep "^test" | sort -t | head -5   # Recent test files
ls | where "size > 1000" | sort -S      # Large files sorted

# Math
seq 100 | sum                        # 1+2+...+100 = 5050
ls | select size | avg               # Average file size
```

---

## Recommended Implementation Order

### Phase 1: Core Pipeline Tools
1. **`tail`** - Mirror of head, trivial
2. **`wc`** - Count lines/words/chars
3. **`sort`** - With `-r`, `-S`, `-t` flags
4. **`grep`** - Pattern matching
5. **`cat`** - File reading + passthrough

### Phase 2: Data Manipulation
6. **`lines`** - Split string into list
7. **`uniq`** - Deduplicate
8. **`cut`** - Field extraction
9. **`tr`** - Character translation
10. **`rev`** - Reverse

### Phase 3: Generation & Math
11. **`seq`** - Number sequences
12. **`echo`** - Output strings
13. **`sum`/`avg`/`min`/`max` - Aggregations
14. **`shuf`** - Randomize

### Phase 4: File Operations
15. **`find`** - Directory walking
16. **`basename`/`dirname`** - Path manipulation
17. **`stat`** - File metadata
18. **`touch`/`mkdir`** - File creation

### Phase 5: Advanced
19. **`from-json`/`to-json`** - JSON support
20. **`xargs`** - Command iteration
21. **`awk`/`sed`** - Text processing (subset)
