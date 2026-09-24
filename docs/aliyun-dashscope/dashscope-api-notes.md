# Aliyun DashScope 接口规范研究笔记

> 调研目的：为 token_proxy 评估/实现 DashScope（阿里云百炼 Model Studio）接口兼容提供事实依据。
> 调研日期：2026-09-24（浏览器实抓官方文档，以下所有结论均标注官方来源 URL）。
> 结论先行：DashScope 有**三套并存接口形态**——原生协议、OpenAI 兼容模式（compatible-mode）、OpenAI 兼容 Responses。token_proxy 现有 `"openai"` provider 对兼容模式零代码可用；原生协议需新增转换器，成本集中在 SSE 与 `parameters` 三段嵌套。

---

## 0. 三套接口形态总览

| 形态 | 域名/路径特征 | 覆盖能力 |
|---|---|---|
| DashScope 原生 | `{base}/api/v1/services/aigc/...`、`/api/v1/services/embeddings/...`、`/api/v1/services/rerank/...` | 全量能力（含多模态向量、多模态重排、text_type/instruct/稀疏向量等独占参数） |
| OpenAI 兼容（Chat/Embeddings） | `{base}/compatible-mode/v1/chat/completions`、`/embeddings` | chat、embeddings（仅文本向量） |
| OpenAI 兼容（Responses/Rerank） | `{base}/compatible-mode/v1/responses`、`{base}/compatible-api/v1/reranks` | Responses API、rerank（注意 rerank 是 **compatible-api**，不是 compatible-mode） |

**地域与域名**（业务空间专属新域名，老域名仍可用）：

- 华北2（北京）：`https://{WorkspaceId}.cn-beijing.maas.aliyuncs.com`（老：`https://dashscope.aliyuncs.com`）
- 新加坡：`https://{WorkspaceId}.ap-southeast-1.maas.aliyuncs.com`（老：`https://dashscope-intl.aliyuncs.com`）
- 中国香港：`https://{WorkspaceId}.cn-hongkong.maas.aliyuncs.com`（老：`https://cn-hongkong.dashscope.aliyuncs.com`）
- 另有 `https://dashscope-us.aliyuncs.com`（弗吉尼亚）
- **各地域 API Key 不通用**

来源：
- <https://help.aliyun.com/zh/model-studio/compatibility-of-openai-with-dashscope>（OpenAI 兼容总览）
- <https://help.aliyun.com/zh/model-studio/qwen-api-via-dashscope>（DashScope API 参考，chat 原生协议）

---

## 1. 文本生成（Chat）— DashScope 原生协议

来源：<https://help.aliyun.com/zh/model-studio/qwen-api-via-dashscope>（2026-09-22 更新版）

### 1.1 端点

- 纯文本模型（qwen-plus 等）：`POST {base}/api/v1/services/aigc/text-generation/generation`
- 多模态模型（qwen3.7-plus / qwen3-vl-plus 等）：`POST {base}/api/v1/services/aigc/multimodal-generation/generation`
- SDK base_url：`dashscope.base_http_api_url = "https://{WorkspaceId}.cn-beijing.maas.aliyuncs.com/api/v1"`

### 1.2 请求体骨架（与 OpenAI 最大结构差异）

HTTP 调用时为三段嵌套：**`{model, input:{messages,...}, parameters:{...}}`**，所有采样/生成参数都放 `parameters`：

```
model            string 必选
input.messages   array  必选（HTTP 时包在 input 里）
parameters.*     其余全部参数
```

### 1.3 parameters 参数清单

| 类别 | 参数 | 备注 |
|---|---|---|
| 采样 | `temperature` [0,2)、`top_p` (0,1]、`top_k`、`repetition_penalty`、`presence_penalty` [-2,2]、`seed`、`stop` | `top_k`、`repetition_penalty` 为 OpenAI 所无；**无 `frequency_penalty`** |
| 思考 | `enable_thinking`、`preserve_thinking`、`thinking_budget`、`reasoning_effort`、`clear_thinking` | 思考内容经响应的 `reasoning_content` 返回；qwen3.8 系 `reasoning_effort` 与 `thinking_budget` 互斥（自动互转：low=4096 / medium=16384 / xhigh=262144） |
| 输出长度 | `max_tokens`（即将废弃）→ `max_completion_tokens`（含思维链） | 语义随模型不同，详见原文 |
| 输出格式 | `response_format`（text / json_object / json_schema） | |
| 流式 | `stream`、`incremental_output` | **`incremental_output` 为 DashScope 特有**；思考模型强制 true；HTTP 流式须加请求头 `X-DashScope-SSE: enable` |
| 工具 | `tools`、`tool_choice`（auto/none/指定函数；Qwen 不支持 required）、`parallel_tool_calls`、`tool_stream` | 用 tools 必须 `result_format="message"`；qwen-vl/qwen-audio 不支持 tools |
| 联网 | `enable_search`、`search_options` | OpenAI 无对应物 |
| VL | `vl_high_resolution_images`、`vl_enable_image_hw_output` | 图像像素上限策略 |
| 其他 | `result_format`（text/message，推荐 message）、`n`(1-4，仅部分模型)、`logprobs`/`top_logprobs`、`enable_code_interpreter`、`skill`(qwen-doc-turbo) | |

特殊请求头：

- `X-DashScope-SSE: enable` —— HTTP 流式开关
- `X-DashScope-DataInspection: {"input":"cip","output":"cip"}` —— 内容安全护栏

### 1.4 图像输入（多模态消息格式）

`content` 为**类型化对象数组**（非字符串），且多模态响应的 content 也是 array：

```python
messages = [
  {'role': 'system', 'content': [{'text': 'You are a helpful assistant.'}]},
  {'role': 'user',   'content': [{'text': ...}, {'image': 'https://...'}]},
  # 元素类型：{text} / {image} / {video} / {audio}
]
```

### 1.5 响应对象

```json
{
  "status_code": 200, "request_id": "...", "code": "", "message": "",
  "output": {
    "choices": [{ "finish_reason": "stop",
      "message": { "role": "assistant", "content": "...",
                   "reasoning_content": "...", "tool_calls": [...] } }]
  },
  "usage": { "input_tokens": 22, "output_tokens": 17, "total_tokens": 39 }
}
```

关键差异：**usage 键名为 `input_tokens`/`output_tokens`/`total_tokens`**（OpenAI 为 `prompt_tokens`/`completion_tokens`/`total_tokens`）；顶层是 `output`（OpenAI 为 `choices`）；`finish_reason` 取值 null/stop/length/tool_calls（语义同 OpenAI）。

---

## 2. 文本与多模态向量化（Embedding）

来源：<https://help.aliyun.com/zh/model-studio/embedding>（#dashscope-h4 锚点章节）

### 2.1 文本向量 — 两种调用方式

**OpenAI 兼容**（token_proxy 现有 openai provider 可直接透传）：

```
POST {base}/compatible-mode/v1/embeddings
{"model": "qwen3.7-text-embedding", "input": "衣服的质量杠杠的"}
```

**DashScope 原生**：

```
POST {base}/api/v1/services/embeddings/text-embedding/text-embedding
{"model": "qwen3.7-text-embedding", "input": {"texts": ["衣服的质量杠杠的"]}}
```

- 兼容接口支持 `dimensions`；**原生独占参数**：`text_type`（query/document）、`instruct`（任务指令，英文）、`output_type`（dense / sparse / dense&sparse）
- 代理注意：`input` 结构不同（字符串/数组 vs `{texts:[...]}` 嵌套）

### 2.2 多模态向量 — 仅 DashScope 原生，**不支持 OpenAI 兼容接口**

```
POST {base}/api/v1/services/embeddings/multimodal-embedding/multimodal-embedding
{"model": "qwen3-vl-embedding",
 "input": {"contents": [{"text": "..."}, {"image": "..."}, {"video": "..."}]},
 "parameters": {"enable_fusion": true, "dimension": 1024}}
```

- 独立向量 vs 融合向量（`enable_fusion`）两类；qwen3-vl-embedding 支持两者
- 文本模型主力：qwen3.7-text-embedding（256~2560 维，批 20，128K 上下文）、text-embedding-v4（Qwen3-Embedding 系列，64~2048 维，批 10）

---

## 3. 重排序（Rerank）

来源：<https://help.aliyun.com/zh/model-studio/rerank>（#dashscope-h4 锚点章节）

### 3.1 文本重排（qwen3-rerank）— "OpenAI 兼容"形态

**注意域名是 `compatible-api`，不是 `compatible-mode`**：

```
POST {base}/compatible-api/v1/reranks
Authorization: Bearer $DASHSCOPE_API_KEY
{"model": "qwen3-rerank",
 "query": "什么是重排序模型",
 "documents": ["...", "..."],
 "top_n": 2,
 "instruct": "Retrieve semantically similar text."}
```

这是 Jina/Cohere 风格自定义端点（OpenAI 官方并无 rerank API），仅认证方式与 SDK 复用 OpenAI 生态，非流式纯 JSON。

**DashScope 原生**（text rerank 的 body 是**扁平**的，无 parameters 嵌套）：

```
POST {base}/api/v1/services/rerank/text-rerank/text-rerank
{"model": "qwen3-rerank", "query": "...", "documents": ["..."],
 "top_n": 2, "return_documents": true}
```

- `instruct` 策略：默认问答检索 `"Given a web search query, retrieve relevant passages that answer the query."`；语义相似 `"Retrieve semantically similar text."`

### 3.2 多模态重排（qwen3-vl-rerank）— 仅 DashScope 原生，**不支持 OpenAI 兼容接口**

body 结构与文本 rerank **不对称**（此处才有 input/parameters 嵌套）：

```
POST {base}/api/v1/services/rerank/text-rerank/text-rerank
{"model": "qwen3-vl-rerank",
 "input": {"query": {"text": "..."},
           "documents": [{"text": "..."}, {"image": "..."}, {"video": "..."}]},
 "parameters": {"return_documents": true, "top_n": 2, "fps": 1.0}}
```

### 3.3 模型与限额（北京，2026-09-03 更新版数据）

| 模型 | 最大文档数 | 单条最大 Token | 单次请求 Token 上限 |
|---|---|---|---|
| qwen3.7-text-rerank | 500 | 30,000 | 120,000（建议） |
| qwen3-rerank | 500 | 4,000 | 120,000 |
| qwen3-vl-rerank | 文本100/图片40/视频4 | 8,000 | 120,000 |
| gte-rerank-v2 | 500 | 4,000 | 30,000（gte-rerank 将于 2026-05-30 下线） |

超限返回 HTTP 400，不截断。

---

## 4. 与 OpenAI 接口差异速查（实现转换器时的映射面）

| 维度 | OpenAI | DashScope 原生 |
|---|---|---|
| 请求骨架 | 平铺 `{model, messages, temperature, ...}` | `{model, input:{messages}, parameters:{...}}`（chat/embedding 多模态/rerank 多模态）；**例外：文本 rerank 扁平** |
| 多模态消息 | `content: [{type:"text",text},{type:"image_url",...}]` | `content: [{text},{image},{video},{audio}]` |
| 流式 | `stream:true`，标准 chunk 事件 | 另需头 `X-DashScope-SSE: enable` + `incremental_output`；事件在 `output.choices[].delta` |
| 思考 | `reasoning`/`reasoning_content`（兼容模式） | `reasoning_content`（在 output.message 内） |
| usage | `prompt_tokens`/`completion_tokens` | `input_tokens`/`output_tokens` |
| 顶层响应 | `choices` | `output.choices`（另有 `request_id`/`code`/`message`） |
| 联网搜索 | 无（server 端 tools） | `enable_search`/`search_options` |
| 采样 | `frequency_penalty`/`presence_penalty` | 仅 `presence_penalty` + `repetition_penalty`，另有 `top_k` |

---

## 5. 对 token_proxy 的实现启示（结合代码调研，2026-09-24）

1. **兼容模式 = 零代码**：现有 `"openai"` provider（`crates/token_proxy_runtime/src/proxy/server/routes.rs` 的 `(OpenaiChat, "openai")` 恒等直传 + `UpstreamConfig` 任意 base_url）配 `compatible-mode/v1` 即可用 chat/responses/`/v1/embeddings`。
2. **原生路径透传会落错 URL**：`/v1/services/...` 走 `resolve_formatless_plan` 兜底透传（`proxy/server/dispatch.rs:246`），出站 URL 为 `base_url + 原路径` 纯拼接（my_url_compose 移除了猜测式修正，见 `token_proxy_config/src/types.rs:652` 注释）。原生 chat 域名（`/api/v1/...`）与兼容域名（`/compatible-mode/v1/...`）前缀不同，**一个上游无法同时服务两种形态**，url_compose 的 prefix 是家族级一维的（`my_url_compose/compose.rs`）。
3. **rerank 当前完全不支持**：无 `/rerank`、`/reranks` 路由；兼容端点在 `compatible-api` 家族，需扩展 url_compose 家族或新增路径识别。
4. **若做原生协议 provider（方案 B）**：按 gemini 模板新增 FormatTransform 8 变体 + `dashscope_compat` 转换模块 + SSE 重排 + usage 键名映射（否则统计模块取不到 token 数），详见会话分析；`X-DashScope-SSE` 头注入参照 `upstream/prepare.rs` 的 xai 头处理模式。
5. **已实现（2026-09-24）——最终采用"原生透传 provider"路线而非 §5.4 转换器**：`/v1/services` 前缀精确命中且存在 dashscope 上游即纯透传（不做格式转换/usage 解析），出站由 `url_compose.dashscope` 家族纯拼接；注入标记名 **MY-DASHSCOPE-PASSTHROUGH**，恢复手册见上层 `docs/项目维护/260924-01-dashscope原生透传/`；兼容模式（§0/§2.1）照旧零代码。

---

## 附：官方来源索引

| 主题 | URL | 抓取方式/日期 |
|---|---|---|
| DashScope API 参考（chat 原生协议全量参数） | <https://help.aliyun.com/zh/model-studio/qwen-api-via-dashscope> | WebBridge 浏览器实抓 2026-09-24 |
| 文本与多模态向量化 | <https://help.aliyun.com/zh/model-studio/embedding> | WebBridge 浏览器实抓 2026-09-24 |
| 重排序 | <https://help.aliyun.com/zh/model-studio/rerank> | WebBridge 浏览器实抓 2026-09-24 |
| OpenAI 兼容总览 | <https://www.alibabacloud.com/help/zh/model-studio/compatibility-of-openai-with-dashscope> | 网页 2026-09-24 |
| OpenAI 兼容 Responses | <https://www.alibabacloud.com/help/en/model-studio/compatibility-with-openai-responses-api> | 网页 2026-09-24 |
| DeepSeek 等第三方模型接入页（三形态对照） | <https://help.aliyun.com/zh/model-studio/deepseek-api> | 网页 2026-09-24 |
