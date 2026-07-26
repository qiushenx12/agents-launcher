const IMAGE_EXTENSIONS = new Set([
  'png', 'jpg', 'jpeg', 'gif', 'webp', 'bmp', 'svg', 'ico', 'tiff', 'tif', 'avif', 'heic',
])

export const MAX_CONVERSATION_IMAGE_ATTACHMENTS = 5

const IMAGE_MIME_EXTENSIONS: Record<string, string> = {
  'image/png': 'png',
  'image/jpeg': 'jpg',
  'image/gif': 'gif',
  'image/webp': 'webp',
  'image/bmp': 'bmp',
}

export function fileExtension(path: string): string {
  const base = path.replace(/[/\\]$/, '').split(/[/\\]/).pop() ?? ''
  const dot = base.lastIndexOf('.')
  return dot > 0 ? base.slice(dot + 1).toLowerCase() : ''
}

export function basename(path: string): string {
  return path.replace(/[/\\]$/, '').split(/[/\\]/).pop() ?? path
}

export function isImagePath(path: string): boolean {
  return IMAGE_EXTENSIONS.has(fileExtension(path))
}

function normalizeSeparators(path: string): string {
  return path.replace(/[/\\]+$/, '').replace(/\\/g, '/')
}

export function isUnderProject(path: string, projectPath: string): boolean {
  const n = normalizeSeparators(path).toLowerCase()
  const p = normalizeSeparators(projectPath).toLowerCase()
  return n === p || n.startsWith(p + '/')
}

function relativeToProject(path: string, projectPath: string): string {
  const n = normalizeSeparators(path)
  const p = normalizeSeparators(projectPath)
  return n.slice(p.length + 1)
}

function quotePath(path: string): string {
  return `"${path.replace(/"/g, '\\"')}"`
}

export function formatConversationDropPath(
  path: string,
  projectPath: string | null,
  mode: 'relative' | 'filename',
): string {
  if (!projectPath || !isUnderProject(path, projectPath)) {
    return quotePath(normalizeSeparators(path))
  }
  if (mode === 'filename') return quotePath(basename(path))
  return quotePath(relativeToProject(path, projectPath))
}

export function partitionPathsByImageLimit(
  paths: string[],
  currentImageCount: number,
  limit: number = MAX_CONVERSATION_IMAGE_ATTACHMENTS,
): { accepted: string[]; rejected: string[] } {
  const accepted: string[] = []
  const rejected: string[] = []
  let imageCount = currentImageCount
  for (const path of paths) {
    if (isImagePath(path)) {
      if (imageCount >= limit) {
        rejected.push(path)
        continue
      }
      imageCount += 1
    }
    accepted.push(path)
  }
  return { accepted, rejected }
}

export function imageMimeToExtension(mime: string): string | null {
  return IMAGE_MIME_EXTENSIONS[mime.toLowerCase()] ?? null
}

export function arrayBufferToBase64(buffer: ArrayBuffer): string {
  const bytes = new Uint8Array(buffer)
  const chunkSize = 0x8000
  let binary = ''
  for (let offset = 0; offset < bytes.length; offset += chunkSize) {
    const chunk = bytes.subarray(offset, offset + chunkSize)
    binary += String.fromCharCode(...chunk)
  }
  return btoa(binary)
}
