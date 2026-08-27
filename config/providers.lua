-- The built-in provider catalog.
--
-- Not a data file with a parser of its own: this is an ordinary axum config, run through the
-- same VM and the same registrar as `~/.config/axum/init.lua`. A user file that declares
-- `axum.provider("groq", ...)` replaces this one's entry by the same rule that makes any
-- registration idempotent, and there is exactly one loading mechanism to understand.
--
-- Nothing in Rust knows any vendor's name, URL or environment variable. Adding a provider is a
-- call here, or in a user's own file, and needs no rebuild either way.
--
-- `api` names a wire protocol, not a vendor. Most entries speak `openai-completions`, and the
-- surprises are the point: Fireworks, GitHub Copilot, MiniMax, OpenCode Go and Vercel all speak
-- Anthropic's Messages API, while OpenAI itself speaks Responses.
--
-- `base_url` is omitted where the endpoint comes from configuration — a Bedrock region, an
-- Azure resource, a Vertex project, a Cloudflare account. Those declare `compat` explicitly,
-- because there is no host to infer a dialect from until they are configured.
--
-- Model lists are representative, not exhaustive. Costs are dollars per million tokens.

axum.provider("anthropic", {
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

axum.provider("fireworks", {
  name = "Fireworks",
  api = "anthropic-messages",
  base_url = "https://api.fireworks.ai/inference",
  auth = { kind = "api-key", vars = { "FIREWORKS_API_KEY" } },
  models = {
    { id = "accounts/fireworks/models/kimi-k2-instruct", name = "Kimi K2", context_window = 131072, max_tokens = 16384, cost = { input = 0.6, output = 2.5 } },
  },
})

axum.provider("github-copilot", {
  name = "GitHub Copilot",
  api = "anthropic-messages",
  base_url = "https://api.individual.githubcopilot.com",
  auth = { kind = "api-key", vars = { "COPILOT_GITHUB_TOKEN", "GITHUB_TOKEN" } },
  models = {
    { id = "claude-sonnet-4.5", name = "Claude Sonnet 4.5", context_window = 200000, max_tokens = 64000, reasoning = true },
  },
})

axum.provider("kimi-coding", {
  name = "Kimi For Coding",
  api = "anthropic-messages",
  base_url = "https://api.kimi.com/coding",
  auth = { kind = "api-key", vars = { "KIMI_API_KEY" } },
  models = {
    { id = "kimi-k2-turbo-preview", name = "Kimi K2 Turbo", context_window = 262144, max_tokens = 32768 },
  },
})

axum.provider("minimax", {
  name = "MiniMax",
  api = "anthropic-messages",
  base_url = "https://api.minimax.io/anthropic",
  auth = { kind = "api-key", vars = { "MINIMAX_API_KEY" } },
  models = {
    { id = "MiniMax-M2", name = "MiniMax M2", context_window = 204800, max_tokens = 131072, reasoning = true, cost = { input = 0.3, output = 1.2 } },
  },
})

axum.provider("minimax-cn", {
  name = "MiniMax CN",
  api = "anthropic-messages",
  base_url = "https://api.minimaxi.com/anthropic",
  auth = { kind = "api-key", vars = { "MINIMAX_CN_API_KEY" } },
  models = {
    { id = "MiniMax-M2", name = "MiniMax M2", context_window = 204800, max_tokens = 131072, reasoning = true },
  },
})

axum.provider("opencode-go", {
  name = "OpenCode Go",
  api = "anthropic-messages",
  base_url = "https://opencode.ai/go",
  auth = { kind = "api-key", vars = { "OPENCODE_API_KEY" } },
  models = {
    { id = "claude-sonnet-4-5", name = "Claude Sonnet 4.5", context_window = 200000, max_tokens = 64000, reasoning = true },
  },
})

axum.provider("vercel-ai-gateway", {
  name = "Vercel AI Gateway",
  api = "anthropic-messages",
  base_url = "https://ai-gateway.vercel.sh",
  auth = { kind = "api-key", vars = { "AI_GATEWAY_API_KEY", "VERCEL_AI_GATEWAY_KEY" } },
  models = {
    { id = "anthropic/claude-sonnet-4.5", name = "Claude Sonnet 4.5", context_window = 200000, max_tokens = 64000, reasoning = true, cost = { input = 3.0, output = 15.0 } },
  },
})

axum.provider("openai", {
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

axum.provider("xai", {
  name = "xAI",
  api = "openai-responses",
  base_url = "https://api.x.ai/v1",
  auth = { kind = "api-key", vars = { "XAI_API_KEY" } },
  models = {
    { id = "grok-4", name = "Grok 4", context_window = 256000, max_tokens = 64000, reasoning = true, cost = { input = 3.0, output = 15.0 } },
    { id = "grok-code-fast-1", name = "Grok Code Fast", context_window = 256000, max_tokens = 64000, cost = { input = 0.2, output = 1.5 } },
  },
})

axum.provider("azure-openai-responses", {
  name = "Azure OpenAI",
  api = "azure-openai-responses",
  auth = { kind = "api-key", vars = { "AZURE_OPENAI_API_KEY" } },
  models = {
    { id = "gpt-5.1", name = "GPT-5.1", context_window = 400000, max_tokens = 128000, reasoning = true },
  },
})

axum.provider("openai-codex", {
  name = "OpenAI (ChatGPT Plus/Pro)",
  api = "openai-codex-responses",
  base_url = "https://chatgpt.com/backend-api",
  auth = { kind = "oauth", service = "ChatGPT" },
  models = {
    { id = "gpt-5.1-codex", name = "GPT-5.1 Codex", context_window = 400000, max_tokens = 128000, reasoning = true },
  },
})

axum.provider("google", {
  name = "Google",
  api = "google-generative-ai",
  base_url = "https://generativelanguage.googleapis.com/v1beta",
  auth = { kind = "api-key", vars = { "GEMINI_API_KEY", "GOOGLE_API_KEY" } },
  models = {
    { id = "gemini-3-pro", name = "Gemini 3 Pro", context_window = 1048576, max_tokens = 65536, reasoning = true, input = { "text", "image" }, cost = { input = 1.25, output = 10.0 } },
    { id = "gemini-3-flash", name = "Gemini 3 Flash", context_window = 1048576, max_tokens = 65536, reasoning = true, input = { "text", "image" }, cost = { input = 0.3, output = 2.5 } },
  },
})

axum.provider("google-vertex", {
  name = "Google Vertex AI",
  api = "google-vertex",
  auth = { kind = "google-adc" },
  models = {
    { id = "gemini-3-pro", name = "Gemini 3 Pro", context_window = 1048576, max_tokens = 65536, reasoning = true, input = { "text", "image" } },
  },
})

axum.provider("amazon-bedrock", {
  name = "Amazon Bedrock",
  api = "bedrock-converse-stream",
  auth = { kind = "aws-sig-v4" },
  models = {
    { id = "anthropic.claude-sonnet-4-5-v1:0", name = "Claude Sonnet 4.5", context_window = 200000, max_tokens = 64000, reasoning = true, input = { "text", "image" } },
  },
})

-- Mistral serves an OpenAI-compatible endpoint alongside its own Conversations API, so it
-- needs no protocol of its own. Pi wrote a separate adapter for the richer one; this takes the
-- dialect twenty other vendors already speak, and works today rather than eventually.
axum.provider("mistral", {
  name = "Mistral",
  api = "openai-completions",
  base_url = "https://api.mistral.ai/v1",
  auth = { kind = "api-key", vars = { "MISTRAL_API_KEY" } },
  models = {
    { id = "mistral-large-latest", name = "Mistral Large", context_window = 131072, max_tokens = 32768, cost = { input = 2.0, output = 6.0 } },
    { id = "codestral-latest", name = "Codestral", context_window = 262144, max_tokens = 32768, cost = { input = 0.3, output = 0.9 } },
  },
})

axum.provider("opencode", {
  name = "OpenCode Zen",
  api = "pi-messages",
  base_url = "https://opencode.ai/zen",
  auth = { kind = "api-key", vars = { "OPENCODE_API_KEY" } },
  models = {
    { id = "claude-sonnet-4-5", name = "Claude Sonnet 4.5", context_window = 200000, max_tokens = 64000, reasoning = true },
  },
})

axum.provider("openrouter", {
  name = "OpenRouter",
  api = "openai-completions",
  base_url = "https://openrouter.ai/api/v1",
  auth = { kind = "api-key", vars = { "OPENROUTER_API_KEY" } },
  compat = { supports_reasoning_effort = true, thinking_format = "openrouter" },
  -- Context, ceiling and price come from OpenRouter's own `/models`, not from memory. The
  -- three models this used to list were a generation and a half behind what the account could
  -- actually reach, which reads from the inside as "OpenRouter is broken".
  models = {
    { id = "anthropic/claude-opus-5", name = "Claude Opus 5", context_window = 1000000, max_tokens = 128000, reasoning = true, cost = { input = 5.0, output = 25.0 } },
    { id = "anthropic/claude-sonnet-4.5", name = "Claude Sonnet 4.5", context_window = 1000000, max_tokens = 64000, reasoning = true, cost = { input = 3.0, output = 15.0 } },
    { id = "openai/gpt-5.1", name = "GPT-5.1", context_window = 400000, max_tokens = 128000, reasoning = true, cost = { input = 1.25, output = 10.0 } },
    { id = "deepseek/deepseek-v4-pro", name = "DeepSeek V4 Pro", context_window = 1048576, max_tokens = 384000, reasoning = true, cost = { input = 0.87, output = 1.74 } },
    { id = "deepseek/deepseek-v4-flash-0731", name = "DeepSeek V4 Flash", context_window = 1310720, max_tokens = 943718, reasoning = true, cost = { input = 0.05, output = 0.10 } },
    { id = "deepseek/deepseek-v3.2", name = "DeepSeek V3.2", context_window = 163840, max_tokens = 147456, reasoning = true, cost = { input = 0.26, output = 0.38 } },
  },
})

axum.provider("groq", {
  name = "Groq",
  api = "openai-completions",
  base_url = "https://api.groq.com/openai/v1",
  auth = { kind = "api-key", vars = { "GROQ_API_KEY" } },
  models = {
    { id = "moonshotai/kimi-k2-instruct", name = "Kimi K2", context_window = 131072, max_tokens = 16384, cost = { input = 1.0, output = 3.0 } },
    { id = "llama-3.3-70b-versatile", name = "Llama 3.3 70B", context_window = 131072, max_tokens = 32768, cost = { input = 0.59, output = 0.79 } },
  },
})

axum.provider("cerebras", {
  name = "Cerebras",
  api = "openai-completions",
  base_url = "https://api.cerebras.ai/v1",
  auth = { kind = "api-key", vars = { "CEREBRAS_API_KEY" } },
  compat = { supports_usage_in_streaming = false },
  models = {
    { id = "qwen-3-coder-480b", name = "Qwen3 Coder 480B", context_window = 131072, max_tokens = 40000, cost = { input = 2.0, output = 2.0 } },
  },
})

axum.provider("deepseek", {
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

axum.provider("ant-ling", {
  name = "Ant Ling",
  api = "openai-completions",
  base_url = "https://api.ant-ling.com/v1",
  auth = { kind = "api-key", vars = { "ANT_LING_API_KEY" } },
  models = {
    { id = "Ling-1T", name = "Ling 1T", context_window = 131072, max_tokens = 32768, reasoning = true },
  },
})

axum.provider("baseten", {
  name = "Baseten",
  api = "openai-completions",
  base_url = "https://inference.baseten.co/v1",
  auth = { kind = "api-key", vars = { "BASETEN_API_KEY" } },
  models = {
    { id = "moonshotai/Kimi-K2-Instruct", name = "Kimi K2", context_window = 131072, max_tokens = 16384 },
  },
})

axum.provider("huggingface", {
  name = "Hugging Face",
  api = "openai-completions",
  base_url = "https://router.huggingface.co/v1",
  auth = { kind = "api-key", vars = { "HF_TOKEN", "HUGGINGFACE_API_KEY" } },
  models = {
    { id = "deepseek-ai/DeepSeek-V3.2", name = "DeepSeek V3.2", context_window = 163840, max_tokens = 65536 },
  },
})

axum.provider("moonshotai", {
  name = "Moonshot AI",
  api = "openai-completions",
  base_url = "https://api.moonshot.ai/v1",
  auth = { kind = "api-key", vars = { "MOONSHOT_API_KEY" } },
  compat = { requires_tool_result_name = true },
  models = {
    { id = "kimi-k2-0905-preview", name = "Kimi K2", context_window = 262144, max_tokens = 32768, cost = { input = 0.6, output = 2.5 } },
  },
})

axum.provider("moonshotai-cn", {
  name = "Moonshot AI CN",
  api = "openai-completions",
  base_url = "https://api.moonshot.cn/v1",
  auth = { kind = "api-key", vars = { "MOONSHOT_API_KEY" } },
  compat = { requires_tool_result_name = true },
  models = {
    { id = "kimi-k2-0905-preview", name = "Kimi K2", context_window = 262144, max_tokens = 32768 },
  },
})

axum.provider("nvidia", {
  name = "NVIDIA",
  api = "openai-completions",
  base_url = "https://integrate.api.nvidia.com/v1",
  auth = { kind = "api-key", vars = { "NVIDIA_API_KEY" } },
  compat = { requires_thinking_as_text = true },
  models = {
    { id = "deepseek-ai/deepseek-v3.1", name = "DeepSeek V3.1", context_window = 131072, max_tokens = 32768, reasoning = true },
  },
})

axum.provider("together", {
  name = "Together",
  api = "openai-completions",
  base_url = "https://api.together.ai/v1",
  auth = { kind = "api-key", vars = { "TOGETHER_API_KEY" } },
  compat = { requires_thinking_as_text = true },
  models = {
    { id = "deepseek-ai/DeepSeek-V3", name = "DeepSeek V3", context_window = 131072, max_tokens = 32768, cost = { input = 1.25, output = 1.25 } },
  },
})

axum.provider("zai", {
  name = "Z.AI",
  api = "openai-completions",
  base_url = "https://api.z.ai/api/coding/paas/v4",
  auth = { kind = "api-key", vars = { "ZAI_API_KEY" } },
  compat = { requires_tool_result_name = true, thinking_format = "zai" },
  models = {
    { id = "glm-4.6", name = "GLM-4.6", context_window = 204800, max_tokens = 131072, reasoning = true, cost = { input = 0.6, output = 2.2 } },
  },
})

axum.provider("zai-coding-cn", {
  name = "Z.AI Coding CN",
  api = "openai-completions",
  base_url = "https://open.bigmodel.cn/api/coding/paas/v4",
  auth = { kind = "api-key", vars = { "ZAI_CODING_CN_API_KEY" } },
  compat = { requires_tool_result_name = true, thinking_format = "zai" },
  models = {
    { id = "glm-4.6", name = "GLM-4.6", context_window = 204800, max_tokens = 131072, reasoning = true },
  },
})

axum.provider("qwen-token-plan", {
  name = "Qwen Token Plan",
  api = "openai-completions",
  base_url = "https://token-plan.ap-southeast-1.maas.aliyuncs.com/compatible-mode/v1",
  auth = { kind = "api-key", vars = { "QWEN_TOKEN_PLAN_API_KEY" } },
  compat = { thinking_format = "qwen" },
  models = {
    { id = "qwen3-coder-plus", name = "Qwen3 Coder Plus", context_window = 1048576, max_tokens = 65536 },
  },
})

axum.provider("qwen-token-plan-cn", {
  name = "Qwen Token Plan CN",
  api = "openai-completions",
  base_url = "https://token-plan.cn-beijing.maas.aliyuncs.com/compatible-mode/v1",
  auth = { kind = "api-key", vars = { "QWEN_TOKEN_PLAN_CN_API_KEY" } },
  compat = { thinking_format = "qwen" },
  models = {
    { id = "qwen3-coder-plus", name = "Qwen3 Coder Plus", context_window = 1048576, max_tokens = 65536 },
  },
})

axum.provider("qwen-token-plan-individual", {
  name = "Qwen Token Plan Individual",
  api = "openai-completions",
  base_url = "https://token-plan.ap-southeast-1.maas.aliyuncs.com/compatible-mode/v1",
  auth = { kind = "api-key", vars = { "QWEN_TOKEN_PLAN_API_KEY" } },
  compat = { thinking_format = "qwen" },
  models = {
    { id = "qwen3-coder-plus", name = "Qwen3 Coder Plus", context_window = 1048576, max_tokens = 65536 },
  },
})

axum.provider("xiaomi", {
  name = "Xiaomi",
  api = "openai-completions",
  base_url = "https://api.xiaomimimo.com/v1",
  auth = { kind = "api-key", vars = { "XIAOMI_API_KEY" } },
  models = {
    { id = "MiMo-VL-7B-RL", name = "MiMo VL 7B", context_window = 131072, max_tokens = 32768, input = { "text", "image" } },
  },
})

axum.provider("xiaomi-token-plan-cn", {
  name = "Xiaomi Token Plan CN",
  api = "openai-completions",
  base_url = "https://token-plan-cn.xiaomimimo.com/v1",
  auth = { kind = "api-key", vars = { "XIAOMI_TOKEN_PLAN_CN_API_KEY" } },
  models = {
    { id = "MiMo-VL-7B-RL", name = "MiMo VL 7B", context_window = 131072, max_tokens = 32768 },
  },
})

axum.provider("xiaomi-token-plan-ams", {
  name = "Xiaomi Token Plan AMS",
  api = "openai-completions",
  base_url = "https://token-plan-ams.xiaomimimo.com/v1",
  auth = { kind = "api-key", vars = { "XIAOMI_TOKEN_PLAN_AMS_API_KEY" } },
  models = {
    { id = "MiMo-VL-7B-RL", name = "MiMo VL 7B", context_window = 131072, max_tokens = 32768 },
  },
})

axum.provider("xiaomi-token-plan-sgp", {
  name = "Xiaomi Token Plan SGP",
  api = "openai-completions",
  base_url = "https://token-plan-sgp.xiaomimimo.com/v1",
  auth = { kind = "api-key", vars = { "XIAOMI_TOKEN_PLAN_SGP_API_KEY" } },
  models = {
    { id = "MiMo-VL-7B-RL", name = "MiMo VL 7B", context_window = 131072, max_tokens = 32768 },
  },
})

axum.provider("cloudflare-workers-ai", {
  name = "Cloudflare Workers AI",
  api = "openai-completions",
  auth = { kind = "api-key", vars = { "CLOUDFLARE_API_TOKEN" } },
  compat = { max_tokens_field = "max_tokens", supports_finish_reason = false },
  models = {
    { id = "@cf/meta/llama-3.3-70b-instruct-fp8-fast", name = "Llama 3.3 70B", context_window = 131072, max_tokens = 32768 },
  },
})

axum.provider("cloudflare-ai-gateway", {
  name = "Cloudflare AI Gateway",
  api = "openai-completions",
  auth = { kind = "api-key", vars = { "CLOUDFLARE_API_TOKEN" } },
  compat = { max_tokens_field = "max_tokens" },
  models = {
    { id = "openai/gpt-5.1", name = "GPT-5.1", context_window = 400000, max_tokens = 128000, reasoning = true },
  },
})

axum.provider("ollama", {
  name = "Ollama",
  api = "openai-completions",
  base_url = "http://localhost:11434/v1",
  auth = { kind = "none" },
  models = {
    { id = "qwen3-coder", name = "Qwen3 Coder", context_window = 262144, max_tokens = 32768 },
    { id = "llama3.3", name = "Llama 3.3", context_window = 131072, max_tokens = 32768 },
  },
})

axum.provider("lmstudio", {
  name = "LM Studio",
  api = "openai-completions",
  base_url = "http://localhost:1234/v1",
  auth = { kind = "none" },
  models = {
    { id = "local-model", name = "Local model", context_window = 131072, max_tokens = 32768 },
  },
})

