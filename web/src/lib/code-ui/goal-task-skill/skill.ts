/**
 * A0-07 curated skill discovery surface for the browser.
 *
 * Live Code UI HTTP for skill search/activation lands in W3-01. Until then the
 * browser consumes this pinned registry (mirroring
 * `CLAUDE_CODE_SKILL_REGISTRY` / `CODEX_SKILL_REGISTRY` /
 * `OPENCODE_SKILL_REGISTRY`) and validates activation the same way
 * `ExecutionControlService::skill_activate` does — no second skill store.
 */

export interface DiscoveredSkill {
  name: string;
  provider: string;
}

export interface SkillSearchQuery {
  skill?: string;
  provider?: string;
}

export interface SkillActivation {
  provider: string;
  name: string;
}

export interface SkillTransport {
  request<T>(path: string, init?: RequestInit): Promise<T>;
}

/** Curated A0-07 discovery rows pinned from `skill_registry_for`. */
export const A0_07_DISCOVERED_SKILLS: readonly DiscoveredSkill[] = [
  { name: "/review", provider: "claude-code" },
  { name: "/security-review", provider: "claude-code" },
  { name: "/simplify", provider: "claude-code" },
  { name: "/review", provider: "codex" },
  { name: "/review", provider: "opencode" },
];

export function discoverSkills(provider?: string): DiscoveredSkill[] {
  if (!provider?.trim()) return [...A0_07_DISCOVERED_SKILLS];
  const needle = provider.trim().toLowerCase();
  return A0_07_DISCOVERED_SKILLS.filter((skill) => skill.provider === needle);
}

export function searchDiscoveredSkills(query: SkillSearchQuery = {}): DiscoveredSkill[] {
  const skillNeedle = query.skill?.trim().toLowerCase();
  const providerNeedle = query.provider?.trim().toLowerCase();
  return A0_07_DISCOVERED_SKILLS.filter((skill) => {
    if (providerNeedle && skill.provider !== providerNeedle) return false;
    if (skillNeedle && skill.name.toLowerCase() !== skillNeedle) return false;
    return true;
  });
}

export function validateSkillActivation(activation: SkillActivation): string | undefined {
  if (!activation.provider.trim()) return "Select a skill provider before activating.";
  if (!activation.name.trim()) return "Select a skill name before activating.";
  const ok = A0_07_DISCOVERED_SKILLS.some(
    (skill) => skill.provider === activation.provider && skill.name === activation.name,
  );
  if (!ok) {
    return `skill '${activation.name}' is not discoverable for provider '${activation.provider}'`;
  }
  return undefined;
}

/** Live skill discovery and leased activation endpoints. */
export function createSkillApi(transport: SkillTransport) {
  return {
    list(query: SkillSearchQuery = {}): Promise<{ items: DiscoveredSkill[] }> {
      const params = new URLSearchParams();
      if (query.provider) params.set("provider", query.provider);
      if (query.skill) params.set("skill", query.skill);
      const suffix = params.toString();
      return transport.request(`/api/code/skills${suffix ? `?${suffix}` : ""}`);
    },
    activate(
      activation: SkillActivation,
      controllerToken: string,
    ): Promise<{ accepted: true; effect?: string; activated?: boolean; message?: string }> {
      return transport.request("/api/code/skills/activate", {
        method: "POST",
        headers: {
          "content-type": "application/json",
          "X-Code-Controller-Token": controllerToken,
        },
        body: JSON.stringify(activation),
      });
    },
  };
}

/**
 * Fixture-backed discovery check mirroring
 * `ExecutionControlService::skill_activate` validation.
 * Live activation posts through `createSkillApi` / goal-task-skill API.
 */
export function validateDiscoveredSkill(activation: SkillActivation): {
  accepted: boolean;
  message: string;
} {
  const error = validateSkillActivation(activation);
  if (error) throw new Error(error);
  return {
    accepted: true,
    message: `Discoverable: ${activation.name} for ${activation.provider} (in-process activation is unsupported until the provider emits SkillEvent)`,
  };
}

/** @deprecated Prefer validateDiscoveredSkill — activation is not a live runtime call yet. */
export const activateDiscoveredSkill = validateDiscoveredSkill;
