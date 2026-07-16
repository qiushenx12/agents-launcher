const CONPTY_RELEASE_ONLY_CHARACTERS = new Set(['‘', '’', '“', '”'])

function win32InputRecord(character: string, keyDown: boolean): string {
  const codePoint = character.codePointAt(0)
  if (codePoint === undefined) return character

  // ConPTY Win32 input mode: CSI Vk ; Sc ; Uc ; Kd ; Cs ; Rc _
  return `\x1b[0;0;${codePoint};${keyDown ? 1 : 0};0;1_`
}

/**
 * The system ConPTY used by portable-pty reports these IME punctuation
 * characters as key-up only when they are sent as plain UTF-8. Codex ignores
 * key-up events. Send explicit Win32 input records so typing and paste both
 * produce the key-down event expected by Codex.
 */
export function encodeCodexConptyInput(data: string): string {
  if (![...data].some(character => CONPTY_RELEASE_ONLY_CHARACTERS.has(character))) {
    return data
  }

  return [...data].map((character) => {
    if (!CONPTY_RELEASE_ONLY_CHARACTERS.has(character)) return character
    return win32InputRecord(character, true) + win32InputRecord(character, false)
  }).join('')
}
