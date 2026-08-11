import type { CodeUiInteractionResponse, JsonValue } from "../types";

export interface UserInputOption {
  id: string;
  label: string;
  description?: string;
}

export interface UserInputQuestion {
  id: string;
  prompt: string;
  header?: string;
  kind: "single" | "text";
  options: UserInputOption[];
  isOther: boolean;
  isSecret: boolean;
}

/** Stable option id the browser client adds when `isOther` is set. */
export const NONE_OF_THE_ABOVE_OPTION_ID = "__none_of_the_above__";
export const NONE_OF_THE_ABOVE_LABEL = "None of the above";

function isRecord(value: JsonValue): value is Record<string, JsonValue> {
  return value !== null && !Array.isArray(value) && typeof value === "object";
}

function readBool(value: JsonValue | undefined): boolean {
  return value === true;
}

function questionList(metadata: JsonValue): JsonValue[] | undefined {
  if (Array.isArray(metadata)) return metadata;
  if (isRecord(metadata) && Array.isArray(metadata.questions)) return metadata.questions;
  return undefined;
}

function parseOption(option: JsonValue): UserInputOption | undefined {
  if (typeof option === "string" && option.trim()) {
    return { id: option.trim(), label: option.trim() };
  }
  if (!isRecord(option)) return undefined;
  // Explicit blank label is invalid even when `id` is set (matches Rust projection).
  if (typeof option.label === "string" && !option.label.trim()) return undefined;
  const label =
    typeof option.label === "string" && option.label.trim()
      ? option.label.trim()
      : typeof option.id === "string" && option.id.trim()
        ? option.id.trim()
        : undefined;
  if (!label) return undefined;
  const id =
    typeof option.id === "string" && option.id.trim() ? option.id.trim() : label;
  const description =
    typeof option.description === "string" && option.description.trim()
      ? option.description
      : undefined;
  return { id, label, description };
}

function parseQuestion(candidate: JsonValue): UserInputQuestion | undefined {
  if (!isRecord(candidate)) return undefined;
  // Keep the wire id verbatim — runtime validates answers against the original id.
  const id = typeof candidate.id === "string" ? candidate.id : "";
  if (!id.trim()) return undefined;
  const prompt =
    typeof candidate.prompt === "string" && candidate.prompt.trim()
      ? candidate.prompt
      : typeof candidate.question === "string" && candidate.question.trim()
        ? candidate.question
        : "";
  if (!prompt) return undefined;

  const rawOptions = Array.isArray(candidate.options) ? candidate.options : [];
  const parsedOptions: UserInputOption[] = [];
  const optionIds = new Set<string>();
  for (const option of rawOptions) {
    const parsed = parseOption(option);
    if (!parsed) continue; // skip blank/malformed options instead of blanking the form
    if (optionIds.has(parsed.id)) continue;
    optionIds.add(parsed.id);
    parsedOptions.push(parsed);
  }

  const explicitKind =
    candidate.kind === "single" || candidate.kind === "text" ? candidate.kind : undefined;
  const kind = explicitKind ?? (parsedOptions.length > 0 ? "single" : "text");
  if (kind === "single" && parsedOptions.length === 0) return undefined;

  const isOther = readBool(candidate.isOther) || readBool(candidate.is_other);
  const isSecret = readBool(candidate.isSecret) || readBool(candidate.is_secret);
  const header = typeof candidate.header === "string" ? candidate.header : undefined;

  return {
    id,
    prompt,
    header,
    kind,
    options: parsedOptions,
    isOther,
    isSecret,
  };
}

/** True when metadata looks like questions were present but none could be parsed. */
export function hasUnparseableQuestions(metadata: JsonValue): boolean {
  const list = questionList(metadata);
  return Boolean(list && list.length > 0 && parseQuestions(metadata).length === 0);
}

export function parseQuestions(metadata: JsonValue): UserInputQuestion[] {
  const list = questionList(metadata);
  if (!list) return [];

  const ids = new Set<string>();
  const questions: UserInputQuestion[] = [];
  for (const candidate of list) {
    const parsed = parseQuestion(candidate);
    if (!parsed || ids.has(parsed.id)) return [];
    ids.add(parsed.id);
    questions.push(parsed);
  }
  return questions;
}

export function selectableOptions(question: UserInputQuestion): UserInputOption[] {
  if (question.kind !== "single") return question.options;
  if (!question.isOther) return question.options;
  if (question.options.some((option) => option.id === NONE_OF_THE_ABOVE_OPTION_ID)) {
    return question.options;
  }
  return [
    ...question.options,
    { id: NONE_OF_THE_ABOVE_OPTION_ID, label: NONE_OF_THE_ABOVE_LABEL },
  ];
}

export function validateAnswers(
  questions: UserInputQuestion[],
  answers: Record<string, string[]>,
): string | undefined {
  if (questions.length === 0) return "The request contains no valid questions.";
  const ids = new Set(questions.map((question) => question.id));
  const unknownIds = Object.keys(answers).filter((id) => !ids.has(id));
  if (unknownIds.length > 0) return `Unknown question id: ${unknownIds.join(", ")}.`;

  for (const question of questions) {
    const values = answers[question.id];
    if (!values || values.length === 0 || !values[0]?.trim()) {
      return `Answer "${question.prompt}" before continuing.`;
    }
    if (question.kind === "single") {
      const options = selectableOptions(question);
      const choice = values[0];
      if (choice === NONE_OF_THE_ABOVE_OPTION_ID) {
        // Follow-up notes are optional (TUI submits the label alone).
        continue;
      }
      if (values.some((value) => !value.trim())) {
        return `Answer "${question.prompt}" before continuing.`;
      }
      if (!options.some((option) => option.id === choice)) {
        return `Answer "${question.prompt}" with one of the provided options.`;
      }
      continue;
    }
    if (values.some((value) => !value.trim())) {
      return `Answer "${question.prompt}" before continuing.`;
    }
  }
  return undefined;
}

export function buildUserInputResponse(
  answers: Record<string, string[]>,
): CodeUiInteractionResponse {
  const normalized: Record<string, string[]> = {};
  for (const [questionId, values] of Object.entries(answers)) {
    if (values[0] === NONE_OF_THE_ABOVE_OPTION_ID) {
      const detail = values[1]?.trim() ?? "";
      normalized[questionId] = detail
        ? [NONE_OF_THE_ABOVE_LABEL, `user_note: ${detail}`]
        : [NONE_OF_THE_ABOVE_LABEL];
      continue;
    }
    normalized[questionId] = values;
  }
  return { answers: normalized };
}

/** Managed Codex adapters cannot resolve browser interaction posts. */
export function browserInteractionRespondSupported(provider: {
  provider: string;
  managed: boolean;
}): boolean {
  return !(provider.managed && provider.provider.toLowerCase() === "codex");
}
