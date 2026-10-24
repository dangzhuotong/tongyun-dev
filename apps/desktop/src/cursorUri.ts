/**
 * Assemble a cursor://file/... URI for opening a folder in Cursor.
 */
export function cursorUri(folderPath: string | null | undefined): string {
  const trimmed = folderPath?.trim() ?? "";
  if (!trimmed) return "";
  const normalized = trimmed.replace(/\\/g, "/");
  const pathPart = normalized.startsWith("/") ? normalized : `/${normalized}`;
  return `cursor://file${encodeURI(pathPart)}`;
}
