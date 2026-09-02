-- The provider catalog: an ordinary magi config, not a data file with its own parser.
--
-- `api` names a wire protocol, not a vendor. `base_url` is omitted where the endpoint comes from
-- configuration. Costs are dollars per million tokens.
--
-- **`discover = true` asks the provider what it offers** instead of listing it here. What comes
-- back is cached under `$XDG_CACHE_HOME/magi/models/` and refreshed daily; it is never written
-- back into this file. A provider that lists `models` is taken at its word and never asked.
--
-- Eight providers, because eight is what gets used. Adding another is a dozen lines; carrying
-- forty that nobody has a key for is a catalog nobody reads.

magi.provider("anthropic", {
  name = "Anthropic",
  api = "anthropic-messages",
  base_url = "https://api.anthropic.com",
  auth = { kind = "api-key", vars = { "ANTHROPIC_API_KEY" } },
  models = {
    { id = "claude-opus-4-6", name = "Claude Opus 4.6", context_window = 200000, max_tokens = 64000, reasoning = true, input = { "text", "image" }, cost = { input = 5.0, output = 25.0, cache_read = 0.5, cache_write = 6.25 } },
    { id = "claude-sonnet-4-5", name = "Claude Sonnet 4.5", context_window = 200000, max_tokens = 64000, reasoning = true, input = { "text", "image" }, cost = { input = 3.0, output = 15.0, cache_read = 0.3, cache_write = 3.75 } },
    { id = "claude-haiku-4-5", name = "Claude Haiku 4.5", context_window = 200000, max_tokens = 64000, reasoning = true, input = { "text", "image" }, cost = { input = 1.0, output = 5.0, cache_read = 0.1, cache_write = 1.25 } },
  },
})

magi.provider("github-copilot", {
  name = "GitHub Copilot",
  api = "anthropic-messages",
  base_url = "https://api.individual.githubcopilot.com",
  auth = { kind = "api-key", vars = { "COPILOT_GITHUB_TOKEN", "GITHUB_TOKEN" } },
  models = {
    { id = "claude-sonnet-4.5", name = "Claude Sonnet 4.5", context_window = 200000, max_tokens = 64000, reasoning = true },
  },
})

magi.provider("openai", {
  name = "OpenAI",
  api = "openai-responses",
  base_url = "https://api.openai.com/v1",
  auth = { kind = "api-key", vars = { "OPENAI_API_KEY" } },
  compat = { max_tokens_field = "max_completion_tokens", supports_developer_role = true, supports_reasoning_effort = true, supports_store = true },
  models = {
    { id = "gpt-5.1", name = "GPT-5.1", context_window = 400000, max_tokens = 128000, reasoning = true, input = { "text", "image" }, cost = { input = 1.25, output = 10.0, cache_read = 0.125 } },
    { id = "gpt-5.1-mini", name = "GPT-5.1 mini", context_window = 400000, max_tokens = 128000, reasoning = true, input = { "text", "image" }, cost = { input = 0.25, output = 2.0 } },
    { id = "gpt-5", name = "GPT-5", context_window = 400000, max_tokens = 128000, reasoning = true, input = { "text", "image" }, cost = { input = 1.25, output = 10.0 } },
  },
})

magi.provider("google", {
  name = "Google",
  api = "google-generative-ai",
  base_url = "https://generativelanguage.googleapis.com/v1beta",
  auth = { kind = "api-key", vars = { "GEMINI_API_KEY", "GOOGLE_API_KEY" } },
  models = {
    { id = "gemini-3-pro", name = "Gemini 3 Pro", context_window = 1048576, max_tokens = 65536, reasoning = true, input = { "text", "image" }, cost = { input = 1.25, output = 10.0 } },
    { id = "gemini-3-flash", name = "Gemini 3 Flash", context_window = 1048576, max_tokens = 65536, reasoning = true, input = { "text", "image" }, cost = { input = 0.3, output = 2.5 } },
  },
})

magi.provider("openrouter", {
  name = "OpenRouter",
  api = "openai-completions",
  base_url = "https://openrouter.ai/api/v1",
  auth = { kind = "api-key", vars = { "OPENROUTER_API_KEY" } },
  compat = { supports_reasoning_effort = true, thinking_format = "openrouter" },
  -- Four hundred and counting. Six were listed here and they were a generation behind, which
  -- reads from the inside as OpenRouter being broken rather than the list being old.
  discover = true,
})

magi.provider("deepseek", {
  name = "DeepSeek",
  api = "openai-completions",
  base_url = "https://api.deepseek.com",
  auth = { kind = "api-key", vars = { "DEEPSEEK_API_KEY" } },
  compat = { thinking_format = "deepseek" },
  models = {
    { id = "deepseek-chat", name = "DeepSeek Chat", context_window = 163840, max_tokens = 65536, cost = { input = 0.28, output = 0.42 } },
    { id = "deepseek-reasoner", name = "DeepSeek Reasoner", context_window = 163840, max_tokens = 65536, reasoning = true, cost = { input = 0.28, output = 0.42 } },
  },
})

magi.provider("zai", {
  name = "Z.AI",
  api = "openai-completions",
  base_url = "https://api.z.ai/api/coding/paas/v4",
  auth = { kind = "api-key", vars = { "ZAI_API_KEY" } },
  compat = { requires_tool_result_name = true, thinking_format = "zai" },
  models = {
    { id = "glm-4.6", name = "GLM-4.6", context_window = 204800, max_tokens = 131072, reasoning = true, cost = { input = 0.6, output = 2.2 } },
  },
})

magi.provider("ollama", {
  name = "Ollama",
  api = "openai-completions",
  base_url = "http://localhost:11434/v1",
  auth = { kind = "none" },
  -- Whatever you have pulled, which is the only list that could be right. It answers on
  -- localhost or it does not answer, and a failed ask leaves the cache alone.
  discover = true,
})

