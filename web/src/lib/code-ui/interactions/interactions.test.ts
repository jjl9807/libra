import { describe, expect, it } from "vitest";

import {
  abortResponse,
  approveResponse,
  browserInteractionRespondSupported,
  buildApprovalResponse,
  buildUserInputResponse,
  denyResponse,
  hasUnparseableQuestions,
  parseQuestions,
  readApprovalMetadata,
  sandboxApprovalFixture,
  selectableOptions,
  toolApprovalFixture,
  userInputFixture,
  validateAnswers,
} from ".";
import type { JsonValue } from "../types";

describe("Code UI interaction helpers", () => {
  it("allows managed Codex browser respond after W3-07 ownership forwarding", () => {
    expect(
      browserInteractionRespondSupported({ provider: "codex", managed: true }),
    ).toBe(true);
    expect(
      browserInteractionRespondSupported({ provider: "ollama", managed: false }),
    ).toBe(true);
  });

  it("builds tool and sandbox approval fixtures with Rust wire options", () => {
    expect(toolApprovalFixture().options.map((option) => option.id)).toEqual(["approve", "deny", "abort"]);
    expect(toolApprovalFixture().metadata).toMatchObject({ sandbox_label: "workspace sandbox" });
    expect(sandboxApprovalFixture()).toMatchObject({
      kind: "sandbox_approval",
      metadata: { sandbox_label: "outside sandbox" },
    });
  });

  it("builds approve, deny, and abort responses", () => {
    expect(approveResponse("accept_all")).toEqual({
      approved: true,
      selectedOption: "approve",
      applyToFuture: "accept_all",
      answers: {},
    });
    expect(denyResponse("decline_all")).toMatchObject({ approved: false, selectedOption: "deny" });
    expect(abortResponse()).toMatchObject({ selectedOption: "abort", applyToFuture: "no" });
    expect(buildApprovalResponse({ selectedOption: "approve", applyToFuture: "no" })).toMatchObject({
      approved: true,
      answers: {},
    });
    expect(
      buildApprovalResponse({ selectedOption: "deny", applyToFuture: "accept_all" }),
    ).toMatchObject({ selectedOption: "deny", applyToFuture: "no" });
    expect(
      buildApprovalResponse({ selectedOption: "approve", applyToFuture: "decline_all" }),
    ).toMatchObject({ selectedOption: "approve", applyToFuture: "no" });
    expect(
      buildApprovalResponse({ selectedOption: "abort", applyToFuture: "accept_all" }),
    ).toMatchObject({ selectedOption: "abort", applyToFuture: "no" });
  });

  it("parses and validates complete multi-question answers", () => {
    const questions = parseQuestions(userInputFixture().metadata);
    const answers = {
      risk_profile: ["low"],
      additional_context: ["Keep the rollout local."],
      deploy_token: ["secret-token"],
    };

    expect(questions).toHaveLength(3);
    expect(questions[0]?.isOther).toBe(true);
    expect(questions[2]?.isSecret).toBe(true);
    expect(validateAnswers(questions, answers)).toBeUndefined();
    expect(buildUserInputResponse(answers)).toEqual({ answers });
  });

  it("fails closed for missing, empty, and unknown answers", () => {
    const questions = parseQuestions(userInputFixture().metadata);

    expect(validateAnswers(questions, { risk_profile: ["low"] })).toMatch(/additional context/i);
    expect(validateAnswers(questions, {
      risk_profile: ["low"],
      additional_context: [" "],
      deploy_token: ["x"],
    })).toMatch(/additional context/i);
    expect(validateAnswers(questions, {
      risk_profile: ["low"],
      additional_context: ["ready"],
      deploy_token: ["x"],
      unexpected: ["value"],
    })).toMatch(/unknown question id/i);
  });

  it("allows None of the above without notes and prefixes optional user_note", () => {
    const questions = parseQuestions(userInputFixture().metadata);
    expect(validateAnswers(questions, {
      risk_profile: ["__none_of_the_above__"],
      additional_context: ["ok"],
      deploy_token: ["tok"],
    })).toBeUndefined();
    expect(buildUserInputResponse({
      risk_profile: ["__none_of_the_above__"],
      additional_context: ["ok"],
      deploy_token: ["tok"],
    }).answers.risk_profile).toEqual(["None of the above"]);
    expect(buildUserInputResponse({
      risk_profile: ["__none_of_the_above__", "Use a custom profile"],
      additional_context: ["ok"],
      deploy_token: ["tok"],
    }).answers.risk_profile).toEqual(["None of the above", "user_note: Use a custom profile"]);
  });

  it("parses the TUI-backed user-input metadata array shape", () => {
    const metadata = [
      {
        id: "risk",
        header: "Risk",
        question: "Pick a risk profile",
        isOther: true,
        isSecret: false,
        options: [{ label: "Low", description: "Safer" }, { label: "High", description: "Faster" }],
      },
      {
        id: "token",
        header: "Secret",
        question: "Deploy token",
        isOther: false,
        isSecret: true,
        options: null,
      },
    ] as JsonValue;
    const questions = parseQuestions(metadata);

    expect(questions).toEqual([
      expect.objectContaining({
        id: "risk",
        prompt: "Pick a risk profile",
        kind: "single",
        isOther: true,
        options: [
          { id: "Low", label: "Low", description: "Safer" },
          { id: "High", label: "High", description: "Faster" },
        ],
      }),
      expect.objectContaining({
        id: "token",
        prompt: "Deploy token",
        kind: "text",
        isSecret: true,
      }),
    ]);
  });

  it("reads camelCase sandboxLabel from TUI approval metadata", () => {
    expect(readApprovalMetadata({
      cwd: "/repo",
      sandboxLabel: "outside sandbox",
      networkAccess: "full",
    })).toEqual({
      command: undefined,
      cwd: "/repo",
      reason: undefined,
      sandboxLabel: "outside sandbox",
    });
  });

  it("honors explicit isOther:false and does not invent None of the above", () => {
    const questions = parseQuestions({
      questions: [
        {
          id: "choice",
          prompt: "Pick one",
          kind: "single",
          isOther: false,
          options: [
            { id: "a", label: "A" },
            { id: "b", label: "B" },
          ],
        },
      ],
    });
    expect(questions[0]?.isOther).toBe(false);
    expect(selectableOptions(questions[0]!).map((option) => option.id)).toEqual(["a", "b"]);
  });

  it("fails closed for malformed question metadata", () => {
    expect(parseQuestions({ questions: [{ id: "", prompt: "Missing id", kind: "text", options: [] }] })).toEqual([]);
    expect(parseQuestions({ questions: [{ id: "choice", prompt: "Choice", kind: "single", options: [] }] })).toEqual([]);
  });

  it("skips blank and duplicate options instead of blanking the form", () => {
    const questions = parseQuestions({
      questions: [
        {
          id: "choice",
          prompt: "Pick one",
          kind: "single",
          options: [
            { id: "a", label: "A", description: "First" },
            { id: "blank", label: "   " },
            { id: "a", label: "A duplicate" },
            { id: "b", label: "B", description: "Second" },
          ],
        },
      ],
    });
    expect(questions).toHaveLength(1);
    expect(questions[0]?.options).toEqual([
      { id: "a", label: "A", description: "First" },
      { id: "b", label: "B", description: "Second" },
    ]);
  });

  it("flags metadata that contains questions but none parse", () => {
    expect(
      hasUnparseableQuestions({
        questions: [{ id: "choice", prompt: "Choice", kind: "single", options: [] }],
      }),
    ).toBe(true);
    expect(hasUnparseableQuestions(userInputFixture().metadata)).toBe(false);
  });

  it("preserves question ids with surrounding whitespace for answer keys", () => {
    const questions = parseQuestions({
      questions: [{ id: " risk ", prompt: "Pick risk", kind: "text", options: [] }],
    });
    expect(questions[0]?.id).toBe(" risk ");
    expect(
      buildUserInputResponse({ " risk ": ["low"] }).answers[" risk "],
    ).toEqual(["low"]);
  });
});
