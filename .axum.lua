-- axum's config: a program, not a data file.
--
-- Settings are assigned, providers are handed to a registrar, and the file returns nothing.
-- Because it is Lua, it can probe the machine it is running on — which is the whole reason a
-- config format was not enough.

axum.model = "anthropic/claude-sonnet-4-5"
axum.theme = "dark"

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
