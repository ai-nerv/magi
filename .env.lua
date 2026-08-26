-- axum's directory environment. Loaded when you `cd` here, unloaded when you leave.
--
-- Shared by every language wing generates, so the paths below are a union: a directory that does
-- not exist in this project costs nothing. A language template can ship its own .env.lua, which
-- wins over this one.

-- The flake's dev shell, without entering one. The slow line here; everything below is instant.
oslo.direnv.nix_develop()

-- This project's own build output, ahead of anything installed, so the binary you just built is
-- the one you get. Idempotent, so a reload does not grow $PATH.
oslo.direnv.path_add("./")
oslo.direnv.path_add("./bin")
oslo.direnv.path_add("./build")
oslo.direnv.path_add("./target")
oslo.direnv.path_add("./zig-out/bin")
oslo.direnv.path_add("./target/debug")
oslo.direnv.path_add("./target/release")

-- Where the checkout is, for scripts that need to find their way back to the top.
oslo.env.set("TOP_HEAD", oslo.sys.pwd())

-- A token in the environment is a token in every child process, and nothing in here needs it.
oslo.env.unset("GITHUB_TOKEN")

-- The commands this repository is driven by. All unload with the directory, so they cannot fire
-- the wrong project's build.
oslo.env.set_alias("_b", "make build")
oslo.env.set_alias("_c", "make compile")
oslo.env.set_alias("_r", "make run")
oslo.env.set_alias("_t", "make test")
oslo.env.set_alias("_v", "make verify")
oslo.env.set_alias("_i", "make install")
