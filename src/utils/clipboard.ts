interface ClipboardWriter {
  writeText(text: string): Promise<void>
}

export interface ClipboardCopyDependencies {
  clipboard?: ClipboardWriter | null
  document?: Document
}

export function copyTextWithDomFallback(text: string, documentRef: Document = document) {
  const textarea = documentRef.createElement('textarea')
  textarea.value = text
  textarea.readOnly = true
  textarea.style.position = 'fixed'
  textarea.style.left = '-9999px'
  textarea.style.top = '0'
  documentRef.body.appendChild(textarea)

  try {
    textarea.select()
    return typeof documentRef.execCommand === 'function' && documentRef.execCommand('copy')
  } finally {
    textarea.remove()
  }
}

export async function copyTextToClipboard(
  text: string,
  dependencies?: ClipboardCopyDependencies,
) {
  const hasClipboardOverride = dependencies
    && Object.prototype.hasOwnProperty.call(dependencies, 'clipboard')
  const clipboard = hasClipboardOverride
    ? dependencies?.clipboard
    : (typeof navigator === 'undefined' ? null : navigator.clipboard)

  if (clipboard?.writeText) {
    try {
      await clipboard.writeText(text)
      return
    } catch {
      // Tauri/WebView clipboard permissions can vary; use the DOM fallback below.
    }
  }

  const documentRef = dependencies?.document
    ?? (typeof document === 'undefined' ? null : document)
  if (!documentRef || !copyTextWithDomFallback(text, documentRef)) {
    throw new Error('Clipboard copy was rejected')
  }
}
