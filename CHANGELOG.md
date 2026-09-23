# Changelog

## [0.5.0] - 2026-09-23

### <!-- 1 -->🐛 Bug Fixes

- Offer tools unlocked mid-prompt
- Let the turn reach the model first

## [0.4.0] - 2026-09-23

### <!-- 0 -->⛰️  Features

- Enforce the request boundary

## [0.3.0] - 2026-09-21

### <!-- 0 -->⛰️  Features

- One main model, a helper for each kind of work
- The dot under the pointer wears its colour
- Melchior and casper get their floats
- Clear what a directory remembers
- Balthasar's float, with tabs
- A tool may put a question to magi
- :rename names a session for good
- Del twice puts a run away, :archives removes it
- Say the model as initials and its name
- Open the history blocks into rules
- Open the boxes into rules
- A role for finding contradictions
- A card for who is asked, and on what terms
- A permission prompt you can read at a glance
- A schema goes as one to a model that decides
- A verdict shape either kind of model can fill
- The mode command and its key are listed
- The judge's view is said plainly
- Ask, edits, auto and locked, with a judge
- Summary, notes and curate fall to memory
- The client can attach from the end
- Say when recording stops and resumes
- A waiting tool says so on its row
- Report by verb, named by role
- Session lifecycle and context fixes
- Ask the upstream that answered last
- --resume-run continues one run by id
- Notes run beside the turn by default
- --logs writes the family's every step
- Notes view, plan fallback, slot split
- Log each layout and helper job
- Layouts and helper jobs in trace, context
- Balthasar lays out every request

### <!-- 1 -->🐛 Bug Fixes

- A fixture the kernel called busy
- Let a job say how much it may reason
- Ask for the roles balthasar now asks about
- Say which roles can actually be run
- A tool row says what was asked for
- Let go of an upstream that failed
- Remember keeps facts, not rules
- Count tokens by kind of character
- The shared tmp is on disk, not in memory
- A lead is told a helper is lost
- A sender is named by role and id
- A cut-off turn ends when the session does
- A permission is always magi's own picker
- Open questions survive a look elsewhere
- The picker offers runs, not subagents
- Leave to write is leave to read
- A snapshot never outgrows its frame
- A helper keeps its own upstream
- Replay in pages; a named run is never lost
- One budget per prompt, awaited on exit
- A resumed run goes on in its transcript
- --attach refuses -p rather than ignore it
- Each session gives its siblings its own
- A wake arrives as melchior's message
- Ask the answer's shape in words
- The recording scribe takes a child's key
- Ask for no reasoning unless configured
- A child is told by its own environment
- Pin stated rules, drop run-bound detail
- Helpers answer without reasoning
- Read every job, report before exit
- Run jobs a layout handed out

### <!-- 6 -->🧪 Testing

- Scripted models keep no notes
- One run, one transcript per agent
- Layouts against a live balthasar

## [0.2.0] - 2026-09-15

### <!-- 0 -->⛰️  Features

- A cost view across models and agents
- A sectioned model card with providers
- Sibling marks rest as a star, light a dot
- Sibling dots flicker like a drive lamp
- Sibling dots rest dim and flash alone
- Draw a change's ground from the palette
- Two bouncing shimmer bands, slower
- Float surfaces, sibling flash, ctrl keys
- Model card, roles, colours and highlighting
- View-only attach and sibling dots
- Deferred tools unlocked by lookup
- Breathing border while a turn runs
- Coordinator model and drivable attach
- Tell a spawned child to report back
- Interactive coordinator wakes on child signal
- Working sweep as separate segments
- Working scan sweeps edges, not the ring
- Magi.may_spawn to pre-authorise spawning
- Crew phase and wake-storm guard
- Headless interrupt and watch reaction
- Phase signals, wake, clickable panel
- --attach <id> opens watching another agent
- The footer crew control opens the tree
- Live status in the agents tree
- A :agents panel drawing the run as a tree
- Give the jail a project-shared /tmp
- Reach casper over its socket door
- Agent and memory reach both doors
- On by default, network kept open
- Landlock walls when bwrap is absent
- Magi.shell gets the seccomp filter too
- A child may be started without leave to spawn
- The two jails mask the same stores
- A reach grant excludes the machine's insides
- A builtin that starts a child, jailed shell
- A tool to start a child agent
- A child's grants narrow to its parent's
- Magi.isolation jails tool commands by grant
- Fill the tools role from magi.tools
- A role says when it cannot be filled
- Look for the memory role by its role name
- The twins agree, and the fake speaks it
- Name the role, not the program
- What a program is for, as a contract
- One typed reply, on every verb
- A comment describes the block
- Every crate takes the workspace denials
- One Lua VM, and it is sandboxed
- A session with no terminal, startable by hand
- Start a child agent of this session
- Move the screen between agents with the arrows
- Learn each peer's id, role and screen
- File scratch under the run and the agent
- Room at the bottom left for the crew control
- Name this session's balthasar, and its own id
- Obey the whole plan, and read history back
- The wheel scrolls the float that has focus
- A click off the float closes it
- One rounded border, scan follows focus, fixed size
- One rounded border, and the scan follows the focus
- A slot that opens, closes and shows it is pressed
- What a session spent, by command or by click
- A permission reaches the memory layer too
- An info pane, with :trace and :graph in it
- Context has suppliers, not one supplier
- Discover, acknowledge and version the surface
- Speak either encoding, read whichever came back
- Tie the memory layer to this process

### <!-- 1 -->🐛 Bug Fixes

- Guess diff colours only for a diff
- Rgb for code and diff colours
- Allow 200 rounds of tool use
- Keep what a tool painted for the screen
- Shimmer one step darker
- Shimmer darkens within the palette
- Wake summary only from inbox, no confabulation
- Keep spawn prompt out of the run grant check
- Tighten the wake prompt
- Resume and keep wait for the store
- Ask the model role's program
- Sessions is core, and a silent resume is not
- A fault names the role, not balthasar
- The memory verbs are magi's to declare
- A served library nobody declared is kept
- A failed read drops the connection
- A slow store is not a refusal
- A write waits, and is not thrown away
- Started means answering, not bound
- A call keeps its own answer
- The schema and the argv agree
- An interrupt ends the ask it stopped
- One profile, and the leaks cwd missed
- Say when a prompt cannot be kept
- The sandbox refuses to half-apply itself
- A prompt is not lost to the hang-up behind it
- A question outranks the surface holding the screen
- A cell is not a square, and a pane wears four
- The arrows moved, then the list was rebuilt
- The spill test raced its siblings for the directory
- Tell a spawn-per-call sibling on every spawn
- A removed package must be forgotten
- A discovered tool must reach the session
- A chained command is not that program
- Guard the grant key a project file can set

### <!-- 2 -->🚜 Refactor

- Cut process/mcp and the peers
- Drop magi.shell, Ops::shell and jail
- Drop magi's read/write/edit floor
- Casper module becomes supplier
- Balthasar decides the cut, magi applies it
- Balthasar holds the session, and nothing else does
- Remove the :graph view

### <!-- 3 -->📚 Documentation

- Casper's socket door, and its jail
- Name casper's file tools accurately
- Magi ships no tools; casper does
- How a tools program is configured
- The roles a configuration may name
- One line for the readiness probe
- The export corpus is not an answer
- The tool door, and the gate that knows it
- Describe the block, drop the argument
- Describe the block, drop the argument
- Two intra-doc links that never resolved
- Describe the block, drop the argument
- Describe the block, drop the argument
- Describe the block, drop the argument
- Cut the argument
- Describe the block, drop the argument
- The gate table names every gate
- Magi keeps no transcript of its own
- Casper's declarations are an installed file
- Link the wiring walkthrough

### <!-- 6 -->🧪 Testing

- Family_live starts its own balthasar
- Scribe_live starts its own balthasar
- A silent balthasar fails, not skips
- A second program fills the tools role
- Pin the role lookup at each site
- A shim fills memory, and a session runs
- The report reads by role
- A corpse socket nothing can inherit
- Drive the desync from both sides
- A call keeps its own answer
- The answer belongs to its own call
- Kill the sleep, not the shell above it
- Bound every read of a sibling's pipe
- The suite leaves no files or processes behind

### <!-- 7 -->⚙️ Miscellaneous Tasks

- Default to deepseek-v4-flash
- Drop gate-deny and unused deps
- Delete the stubs nothing reads
- Ignore balthasar's session stores
- Merge develop to main
- A run can be started by hand
- Merge main to develop

## [0.1.0] - 2026-09-06

### <!-- 1 -->🐛 Bug Fixes

- The dev shell evaluates on aarch64

