"use client";

import { useMemo, useState, type FormEvent } from "react";

import {
  searchDiscoveredSkills,
  type DiscoveredSkill,
  type SkillActivation,
} from "../../../lib/code-ui/goal-task-skill";

export interface SkillSearchPanelProps {
  skills?: DiscoveredSkill[];
  busy?: boolean;
  error?: string;
  lastActivation?: string;
  onActivate(activation: SkillActivation): void | Promise<void>;
}

export function SkillSearchPanel({
  skills,
  busy = false,
  error,
  lastActivation,
  onActivate,
}: SkillSearchPanelProps) {
  const [provider, setProvider] = useState("");
  const [skillName, setSkillName] = useState("");
  const results = useMemo(
    () =>
      skills
        ? skills.filter((skill) => {
            if (provider.trim() && skill.provider !== provider.trim()) return false;
            if (skillName.trim() && skill.name !== skillName.trim()) return false;
            return true;
          })
        : searchDiscoveredSkills({
            provider: provider.trim() || undefined,
            skill: skillName.trim() || undefined,
          }),
    [provider, skillName, skills],
  );

  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const selected = results[0];
    if (!selected) return;
    void onActivate({ provider: selected.provider, name: selected.name });
  };

  return (
    <section aria-label="Skill search panel">
      <h2>Skills</h2>
      <p>Registry data comes from the A0-07 curated discovery surface (validation only until W3-01 skill HTTP).</p>
      <form onSubmit={submit}>
        <label>
          Provider filter
          <input
            aria-label="Skill provider filter"
            value={provider}
            disabled={busy}
            onChange={(event) => setProvider(event.target.value)}
          />
        </label>
        <label>
          Skill filter
          <input
            aria-label="Skill name filter"
            value={skillName}
            disabled={busy}
            onChange={(event) => setSkillName(event.target.value)}
          />
        </label>
        <ul aria-label="Skill search results">
          {results.map((skill) => (
            <li key={`${skill.provider}:${skill.name}`}>
              <button
                type="button"
                disabled={busy}
                onClick={() => void onActivate({ provider: skill.provider, name: skill.name })}
              >
                Validate {skill.name} ({skill.provider})
              </button>
            </li>
          ))}
        </ul>
        <button type="submit" disabled={busy || results.length === 0}>
          Validate first match
        </button>
      </form>
      {lastActivation && <p role="status">{lastActivation}</p>}
      {error && <p role="alert">{error}</p>}
    </section>
  );
}
