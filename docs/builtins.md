# Shell Built-in Commands

This document tracks the built-in commands that Nexus must implement.

## 1. Mandatory Built-ins

These commands **must** be built-in because they modify the shell's own environment.

| Command   | Status | Description |
|-----------|--------|-------------|
| `alias`   | [ ]    | Modifies shell command mapping |
| `bg`      | [ ]    | Resumes job in background |
| `cd`      | [x]    | Changes working directory |
| `command` | [x]    | Bypasses alias/function lookups |
| `eval`    | [x]    | Executes arguments as shell input |
| `exec`    | [ ]    | Replaces the shell process |
| `exit`    | [x]    | Terminates the shell |
| `export`  | [x]    | Marks variables for child processes |
| `fc`      | [ ]    | Fix Command - edits history |
| `fg`      | [ ]    | Brings job to foreground |
| `getopts` | [ ]    | Parses script arguments |
| `hash`    | [ ]    | Modifies command cache |
| `jobs`    | [ ]    | Lists active child processes |
| `kill`    | [ ]    | Signals jobs by Job ID (%1) |
| `read`    | [ ]    | Reads input into variables |
| `set`     | [x]    | Sets shell options/parameters |
| `shift`   | [ ]    | Shifts positional parameters |
| `times`   | [ ]    | Reports accumulated times |
| `trap`    | [ ]    | Traps signals |
| `type`    | [x]    | Describes how shell interprets a name |
| `ulimit`  | [ ]    | Sets resource limits |
| `umask`   | [ ]    | Sets file creation mask |
| `unalias` | [ ]    | Removes aliases |
| `unset`   | [x]    | Unsets shell variables |
| `wait`    | [ ]    | Waits for child processes |

## 2. Performance Built-ins

Standalone utilities often built-in for speed.

| Command   | Status | Description |
|-----------|--------|-------------|
| `basename`| [x]    | Strip directory from path |
| `dirname` | [x]    | Strip filename from path |
| `echo`    | [x]    | Print arguments (with -n, -e, -E) |
| `false`   | [x]    | Return failure (exit 1) |
| `printf`  | [x]    | Formatted output |
| `pwd`     | [x]    | Print working directory |
| `sleep`   | [ ]    | Delay execution |
| `test`/`[`| [x]    | Condition evaluation |
| `true`    | [x]    | Return success (exit 0) |

## 3. POSIX Special Built-ins

These have special error handling - errors cause non-interactive shells to exit.

| Command   | Status | Description |
|-----------|--------|-------------|
| `break`   | [ ]    | Exit from loop |
| `:`       | [x]    | Null command (no-op) |
| `continue`| [ ]    | Continue to next iteration |
| `.`/`source`| [x]  | Execute commands from file |
| `eval`    | [x]    | (see above) |
| `exec`    | [ ]    | (see above) |
| `exit`    | [x]    | (see above) |
| `export`  | [x]    | (see above) |
| `readonly`| [x]    | Mark variables as read-only |
| `return`  | [ ]    | Return from function |
| `set`     | [x]    | (see above) |
| `shift`   | [ ]    | (see above) |
| `times`   | [ ]    | (see above) |
| `trap`    | [ ]    | (see above) |
| `unset`   | [x]    | (see above) |

---

## External Utilities for Integration

These are POSIX utilities that can be integrated for a "fat" or "high-performance" shell, ranked by difficulty.

### Tier 1: Syscall Wrappers (Easy)

These utilities map almost 1:1 to standard C library system calls. They are tiny, require very little code, and make file system operations instantaneous.

| Command   | Status | Description |
|-----------|--------|-------------|
| `chgrp`   | [ ]    | Wrapper around `chown()` syscall |
| `chmod`   | [ ]    | Wrapper around `chmod()` syscall |
| `chown`   | [ ]    | Wrapper around `chown()` syscall |
| `ln`      | [ ]    | Wrapper around `link()` or `symlink()` |
| `mkdir`   | [ ]    | Wrapper around `mkdir()` syscall |
| `rmdir`   | [ ]    | Wrapper around `rmdir()` syscall |
| `rm`      | [ ]    | Wrapper around `unlink()` (recursive adds complexity) |
| `touch`   | [ ]    | Wrapper around `utime()` or `utimensat()` |
| `tty`     | [ ]    | Simple check on `isatty(STDIN_FILENO)` |
| `uname`   | [ ]    | Wrapper around the `uname()` syscall |
| `link`    | [ ]    | Direct syscall wrapper (simpler `ln`) |
| `unlink`  | [ ]    | Direct syscall wrapper (simpler `rm`) |
| `sleep`   | [ ]    | Wrapper around `nanosleep()` |

### Tier 2: Pipe Optimizers (Medium)

These are the most valuable built-ins for scripting performance because they are often used in loops or long pipelines. Implementing these prevents the "fork bomb" effect of heavy text processing scripts.

| Command   | Status | Description |
|-----------|--------|-------------|
| `cat`     | [ ]    | Read file descriptor, write to stdout |
| `head`    | [ ]    | Buffer management, but simple logic |
| `tail`    | [ ]    | Buffer management, but simple logic |
| `wc`      | [ ]    | Count bytes/lines - very easy internally |
| `cut`     | [ ]    | String parsing, common in pipes |
| `uniq`    | [ ]    | Compare current line to previous line |
| `tee`     | [ ]    | Write to stdout and file simultaneously |
| `basename`| [x]    | Simple string manipulation (no I/O) |
| `dirname` | [x]    | Simple string manipulation (no I/O) |
| `cmp`     | [ ]    | Byte-by-byte comparison |

### Tier 3: Ambitious (Complex Logic)

These require significant logic, memory management, and argument parsing. However, ksh93 and busybox implement these to great effect.

| Command   | Status | Description |
|-----------|--------|-------------|
| `cp`      | [ ]    | Permissions across filesystems, recursive, edge cases |
| `mv`      | [ ]    | Similar complexity to `cp` |
| `grep`    | [ ]    | Requires regex library implementation |
| `date`    | [ ]    | Complex time format parsing and output |
| `find`    | [ ]    | Recursive directory tree walking |
| `ls`      | [ ]    | Sorting, column formatting, terminal width detection |
| `printf`  | [x]    | Complex format specifiers (already implemented) |
| `sort`    | [ ]    | Buffering input, efficient sorting algorithms |

### Tier 4: Kitchen Sink (Very Hard)

If you implement these, you aren't just writing a shell; you are writing an Operating System userland.

| Command   | Status | Description |
|-----------|--------|-------------|
| `sed`     | [ ]    | Stream editor - essentially a mini-language |
| `awk`     | [ ]    | Full Turing-complete programming language |
| `vi`/`ed` | [ ]    | Interactive text editors |
| `tar`/`cpio`| [ ]  | Archive handling with complex binary formats |

---

## Implementation Progress

### Implemented
- [x] `cd` - with `-` for OLDPWD, HOME default
- [x] `pwd`
- [x] `echo` - with `-n`, `-e`, `-E` flags
- [x] `exit` - with optional exit code
- [x] `export` - NAME=value and exporting existing vars, prints all when no args
- [x] `unset` - removes vars and env
- [x] `true`, `false`, `:`
- [x] `test`/`[` - file tests, string comparison, numeric comparison, -a/-o
- [x] `type` - shows if builtin, alias, or path
- [x] `printf` - format strings with %s, %d, %x, %o, %e, %c, %b, etc.
- [x] `set` - shell options (-e, -x, -u, -v, -n, -f, -C, -a, -b, -h)
- [x] `source`/`.` - execute file in current shell
- [x] `eval` - execute string as command
- [x] `readonly` - mark variables read-only
- [x] `command` - bypass aliases, -v for type lookup
- [x] `basename` - strip directory from path, optional suffix removal
- [x] `dirname` - strip filename from path

### Phase 2: Job Control (Next)
- [ ] `jobs` - list background jobs
- [ ] `fg` - bring job to foreground
- [ ] `bg` - continue job in background
- [ ] `wait` - wait for background jobs
- [ ] `kill` - with %jobid support

### Phase 3: Scripting
- [ ] `read` - read line into variable
- [ ] `getopts` - parse options
- [ ] `shift` - shift positional params
- [ ] `break`, `continue`, `return`
- [ ] `trap` - signal handling

### Phase 4: Advanced
- [ ] `alias`, `unalias`
- [ ] `exec` - replace shell process
- [ ] `hash` - command cache
- [ ] `fc` - history editing
- [ ] `times`, `ulimit`, `umask`

### Phase 5: Tier 1 Utilities
- [ ] `mkdir`, `rmdir`
- [ ] `rm`, `ln`
- [ ] `chmod`, `chown`, `chgrp`
- [ ] `touch`
- [ ] `sleep`
- [ ] `tty`, `uname`

### Phase 6: Tier 2 Utilities (Optional)
- [ ] `cat`, `head`, `tail`
- [ ] `wc`, `cut`, `uniq`
- [ ] `tee`, `cmp`
