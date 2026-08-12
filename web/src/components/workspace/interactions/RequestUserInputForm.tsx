"use client";

import { useMemo, useState, type FormEvent } from "react";

import {
  NONE_OF_THE_ABOVE_OPTION_ID,
  buildUserInputResponse,
  hasUnparseableQuestions,
  parseQuestions,
  selectableOptions,
  validateAnswers,
} from "../../../lib/code-ui/interactions";
import type {
  CodeUiInteractionRequest,
  CodeUiInteractionResponse,
} from "../../../lib/code-ui/types";

export interface RequestUserInputFormProps {
  interaction: CodeUiInteractionRequest;
  onRespond(interactionId: string, response: CodeUiInteractionResponse): void | Promise<void>;
  onCancel(): void | Promise<void>;
  /** When false, show the questions read-only (respond path unavailable). */
  respondEnabled?: boolean;
}

function errorMessage(cause: unknown): string {
  if (cause && typeof cause === "object" && "message" in cause && typeof cause.message === "string") {
    return cause.message;
  }
  return "Could not deliver these answers. Try again.";
}

export function RequestUserInputForm({
  interaction,
  onRespond,
  onCancel,
  respondEnabled = true,
}: RequestUserInputFormProps) {
  const questions = useMemo(() => parseQuestions(interaction.metadata), [interaction.metadata]);
  const malformed = useMemo(
    () => hasUnparseableQuestions(interaction.metadata),
    [interaction.metadata],
  );
  const [answers, setAnswers] = useState<Record<string, string[]>>({});
  const [error, setError] = useState<string>();
  const [busy, setBusy] = useState(false);

  const run = async (operation: () => Promise<void>) => {
    if (busy) return;
    setBusy(true);
    setError(undefined);
    try {
      await operation();
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setBusy(false);
    }
  };

  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (!respondEnabled) return;
    if (malformed) {
      setError("This user-input request is malformed and cannot be answered from the browser.");
      return;
    }
    const validationError = validateAnswers(questions, answers);
    if (validationError) {
      setError(validationError);
      return;
    }
    void run(async () => {
      await onRespond(interaction.id, buildUserInputResponse(answers));
    });
  };

  return (
    <section aria-label="User input request">
      <p>Interaction: {interaction.id}</p>
      {interaction.title && <h2>{interaction.title}</h2>}
      {interaction.description && <p>{interaction.description}</p>}
      {interaction.prompt && <p>{interaction.prompt}</p>}
      {!respondEnabled && (
        <p role="status">
          This session cannot resolve user-input prompts from the browser right now. Cancel the turn
          here, or retry once the controller can write.
        </p>
      )}
      {malformed && (
        <p role="alert">
          This user-input request is malformed (missing prompts or usable options) and cannot be
          answered from the browser. Cancel the turn and retry with valid questions.
        </p>
      )}
      <form onSubmit={submit}>
        {questions.map((question) => {
          const options = selectableOptions(question);
          const selected = answers[question.id]?.[0] ?? "";
          const otherDetail = answers[question.id]?.[1] ?? "";
          return (
            <fieldset key={question.id} disabled={busy || !respondEnabled || malformed}>
              {question.header && <legend>{question.header}</legend>}
              <label>
                {question.prompt}
                {question.kind === "single" ? (
                  <>
                    <select
                      aria-label={question.prompt}
                      value={selected}
                      onChange={(event) =>
                        setAnswers((current) => ({
                          ...current,
                          [question.id]:
                            event.target.value === NONE_OF_THE_ABOVE_OPTION_ID
                              ? [NONE_OF_THE_ABOVE_OPTION_ID, current[question.id]?.[1] ?? ""]
                              : [event.target.value],
                        }))
                      }
                    >
                      <option value="">Select an option</option>
                      {options.map((option) => (
                        <option key={option.id} value={option.id}>
                          {option.description
                            ? `${option.label} — ${option.description}`
                            : option.label}
                        </option>
                      ))}
                    </select>
                    {selected === NONE_OF_THE_ABOVE_OPTION_ID && (
                      <textarea
                        aria-label={`${question.prompt} details`}
                        value={otherDetail}
                        onChange={(event) =>
                          setAnswers((current) => ({
                            ...current,
                            [question.id]: [NONE_OF_THE_ABOVE_OPTION_ID, event.target.value],
                          }))
                        }
                      />
                    )}
                  </>
                ) : question.isSecret ? (
                  <input
                    type="password"
                    autoComplete="off"
                    aria-label={question.prompt}
                    value={answers[question.id]?.[0] ?? ""}
                    onChange={(event) =>
                      setAnswers((current) => ({
                        ...current,
                        [question.id]: [event.target.value],
                      }))
                    }
                  />
                ) : (
                  <textarea
                    aria-label={question.prompt}
                    value={answers[question.id]?.[0] ?? ""}
                    onChange={(event) =>
                      setAnswers((current) => ({
                        ...current,
                        [question.id]: [event.target.value],
                      }))
                    }
                  />
                )}
              </label>
            </fieldset>
          );
        })}
        {error && <p role="alert">{error}</p>}
        <button type="submit" disabled={busy || !respondEnabled || malformed}>
          Submit answers
        </button>
        <button type="button" disabled={busy} onClick={() => void run(async () => onCancel())}>
          Cancel
        </button>
      </form>
    </section>
  );
}
