// 对照 sub2api 的用量审计：比较「实际发给上游的模型」和「上游响应里的 model」。
// 日期后缀 / -latest 记成变体；xAI 会把 grok-4.x 回成 grok-4.x-build，这种不算不一致。

export type UpstreamResponseModelAudit =
  | { kind: "same" }
  | { kind: "variant"; responseModel: string }
  | { kind: "mismatch"; responseModel: string };

function trimModel(value: string | null | undefined) {
  const trimmed = value?.trim() ?? "";
  return trimmed.length > 0 ? trimmed : null;
}

/** 发给上游的模型。有映射时用映射结果，否则用客户端请求的模型。 */
export function sentUpstreamModel(
  model: string | null | undefined,
  mappedModel: string | null | undefined,
) {
  return trimModel(mappedModel) ?? trimModel(model);
}

function canonicalGrokBuildModel(model: string) {
  switch (model.trim().toLowerCase()) {
    case "grok-4.5":
    case "grok-4.5-latest":
    case "grok-4.5-build":
      return "grok-4.5-build";
    case "grok-4.6":
    case "grok-4.6-latest":
    case "grok-4.6-build":
      return "grok-4.6-build";
    case "grok-4.7":
    case "grok-4.7-latest":
    case "grok-4.7-build":
      return "grok-4.7-build";
    default:
      return null;
  }
}

function modelsMatchForAudit(sent: string, response: string) {
  if (sent.toLowerCase() === response.toLowerCase()) {
    return true;
  }
  const sentGrok = canonicalGrokBuildModel(sent);
  const responseGrok = canonicalGrokBuildModel(response);
  return sentGrok !== null && sentGrok === responseGrok;
}

function normalizeModelVariant(model: string) {
  return model
    .trim()
    .toLowerCase()
    .replace(/-latest$/, "")
    .replace(/-\d{4}-\d{2}-\d{2}$/, "")
    .replace(/-\d{8}$/, "");
}

export function auditUpstreamResponseModel(
  model: string | null | undefined,
  mappedModel: string | null | undefined,
  upstreamResponseModel: string | null | undefined,
): UpstreamResponseModelAudit {
  const responseModel = trimModel(upstreamResponseModel);
  if (!responseModel) {
    return { kind: "same" };
  }
  const sent = sentUpstreamModel(model, mappedModel) ?? "";
  if (modelsMatchForAudit(sent, responseModel)) {
    return { kind: "same" };
  }
  if (sent && normalizeModelVariant(sent) === normalizeModelVariant(responseModel)) {
    return { kind: "variant", responseModel };
  }
  return { kind: "mismatch", responseModel };
}
