const AUTO_TITLE_LIMIT = 80

function cleanTitleCandidate(value: string): string {
  return value
    .trim()
    .replace(/^#{1,6}\s+/, '')
    .replace(/^(?:[-*+]\s+|\d+[.)]\s+)/, '')
    .replace(/\[([^\]]+)\]\([^)]+\)/g, '$1')
    .replace(/[`*_~]/g, '')
    .replace(/\s+/g, ' ')
    .trim()
}

function limitTitle(value: string): string {
  const chars = Array.from(value)
  return chars.length <= AUTO_TITLE_LIMIT
    ? value
    : `${chars.slice(0, AUTO_TITLE_LIMIT - 1).join('').trimEnd()}…`
}

export function splitTaskContent(content: string): { title: string; description?: string } {
  const normalized = content.trim()
  const heading = normalized.match(/^#[ \t]+([^\r\n]+)\r?$/m)

  if (heading?.index !== undefined) {
    const cleanedTitle = cleanTitleCandidate(heading[1])
    const title = limitTitle(cleanedTitle) || 'Task'
    const before = normalized.slice(0, heading.index).trim()
    const after = normalized.slice(heading.index + heading[0].length).trim()
    const description =
      cleanedTitle.length > AUTO_TITLE_LIMIT
        ? normalized
        : [before, after].filter(Boolean).join('\n\n')
    return { title: title || 'Task', description: description || undefined }
  }

  const lines = normalized.split(/\r?\n/)
  const titleLineIndex = lines.findIndex(
    (line) => line.trim() && !/^\s*#{2,}\s/.test(line),
  )
  const selectedLineIndex = titleLineIndex >= 0 ? titleLineIndex : lines.findIndex((line) => line.trim())
  const titleLine = selectedLineIndex >= 0 ? lines[selectedLineIndex] : ''
  const cleanedTitle = cleanTitleCandidate(titleLine)
  const title = limitTitle(cleanedTitle) || 'Task'
  const remaining = lines.filter((_, index) => index !== selectedLineIndex).join('\n').trim()
  const description = cleanedTitle.length > AUTO_TITLE_LIMIT ? normalized : remaining
  return { title, description: description || undefined }
}
