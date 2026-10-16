export type StartCommandSource = {
  start_command?: string;
  profile_start_commands?: Record<string, string>;
};

/** Command the copy button should put on the clipboard for the selected profile. */
export function effectiveStartCommand(
  project: StartCommandSource | null | undefined,
  profile: string | null | undefined,
): string {
  if (!project) return "";
  const name = profile?.trim() ?? "";
  if (name) {
    const specific = project.profile_start_commands?.[name]?.trim();
    if (specific) return specific;
  }
  return project.start_command?.trim() ?? "";
}
