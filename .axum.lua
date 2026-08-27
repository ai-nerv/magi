-- axum's config: a program, not a data file.
--
-- Settings are assigned, providers are handed to a registrar, and the file returns nothing.
-- Because it is Lua, it can probe the machine it is running on — which is the whole reason a
-- config format was not enough.
--
-- WHERE THIS FILE GOES. A `.axum.lua` in a project can *choose* — pick a model, set a theme —
-- but it cannot *declare*. A provider names a URL your whole conversation is sent to and a
-- tool can name a command to run, so a file that arrived with `git clone` is not allowed to
-- add either; axum says so on stderr and carries on without it.
--
-- Declarations go in your own configuration, at `$XDG_CONFIG_HOME/axum/` — `providers.lua`
-- for the block below, `tools/` for tools. `make configs` puts the shipped copies there to
-- edit. If you genuinely want a project to declare things, vouch for it once from there:
--
--     axum.trusted = { "/home/you/work/that-repo" }

axum.model = "anthropic/claude-sonnet-4-5"
axum.theme = "dark"

-- Everything below belongs in `$XDG_CONFIG_HOME/axum/providers.lua`. It is kept here as the
-- worked example of why the config is a program: a provider declared in a loop is the same
-- table as one written out by hand.

-- A provider is a table handed in whole, never a fragment to merge.
axum.provider("my-vllm", {
  name = "My vLLM box",
  api = "openai-completions",
  base_url = "http://10.0.0.7:8000/v1",
  auth = { kind = "none" },
  compat = { supports_usage_in_streaming = false },
  models = {
    { id = "Qwen/Qwen3-Coder-30B", name = "Qwen3 Coder 30B",
      context_window = 262144, max_tokens = 32768 },
  },
})

-- Registration is keyed, so a loop over a directory of machines is idempotent.
for _, box in ipairs({ "alpha", "beta" }) do
  axum.provider("gpu-" .. box, {
    name = "GPU " .. box,
    api = "openai-completions",
    base_url = "http://" .. box .. ".local:8000/v1",
    auth = { kind = "none" },
    models = {
      { id = "qwen3-coder", name = "Qwen3 Coder", context_window = 262144, max_tokens = 32768 },
    },
  })
end
