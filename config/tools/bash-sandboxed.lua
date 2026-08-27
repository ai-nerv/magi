-- `bash`, CONFINED. An example, not registered by default.
--
-- This is the answer to "does axum need namespace sandboxing", and the answer is that it
-- already has it: a process tool names the command that starts its peer, so putting a sandbox
-- in front of that command is a configuration change and not a feature. No new subsystem, no
-- privileged helper, and nothing in the daemon that has to be kept in step with it.
--
-- That matters beyond convenience. Tau made namespaces mandatory and fail-closed, which is a
-- defect on every platform that has none. Here the confinement is the user's choice, expressed
-- where every other tool decision is expressed, and a machine without `bwrap` simply uses the
-- file next to this one.
--
-- TO USE IT: copy over `tools/bash.lua`, and replace the two paths marked below with the
-- directory you want writable. Both must be absolute — a peer is started in the session
-- directory, but bwrap resolves its arguments before that means anything.
--
-- WHAT THIS BUYS, and what it does not:
--
--   * The filesystem is read-only apart from the directory you name. `touch /etc/anything`
--     comes back "Read-only file system", which the model reads as a tool result and works
--     around rather than a crash it cannot see.
--   * No network. `--unshare-net` leaves the peer with loopback only, so a command cannot
--     exfiltrate what it read. Remove it if the work needs to fetch things.
--   * It is NOT a security boundary against a determined attacker with a kernel exploit. It is
--     a boundary against a model that misunderstood an instruction, which is the thing that
--     actually happens.
--
-- Nothing else changes. The peer is the same binary, the protocol is the same, and the model
-- is told the same schema — the peer still declares itself, so this file says nothing about
-- what `bash` does.

axum.tool("bash", {
  transport = {
    kind = "process",
    command = "bwrap",
    args = {
      "--ro-bind", "/", "/",
      "--dev", "/dev",
      "--proc", "/proc",
      -- REPLACE both of these with the directory you want writable.
      "--bind", "/home/you/project", "/home/you/project",
      "--unshare-net",
      -- `axum.self` is this binary: a multi-call executable is its own shell peer.
      axum.self, "ext", "shell",
    },
  },
})
