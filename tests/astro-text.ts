/**
 * Finds user-visible text written straight into an .astro template: text
 * between tags and literal values of the attributes a visitor reads or hears
 * (alt, title, aria-label …). Such text belongs in a catalogue. Markup
 * inside expressions (`{items.map(item => <li>…</li>)}`) is checked too.
 *
 * A small scanner, not a full parser: enough for this site's templates.
 */

const VISIBLE_ATTRIBUTES = new Set([
  'alt',
  'title',
  'placeholder',
  'aria-label',
  'aria-description',
  'aria-placeholder',
]);
const VOID_ELEMENTS = new Set([
  'area',
  'base',
  'br',
  'col',
  'embed',
  'hr',
  'img',
  'input',
  'link',
  'meta',
  'source',
  'track',
  'wbr',
]);

/** The template of an .astro file: no frontmatter, comments, styles or scripts. */
function template(source: string): string {
  return source
    .replace(/^---\n[\s\S]*?\n---\n/, '')
    .replace(/<!--[\s\S]*?-->/g, '')
    .replace(/<(style|script)\b[\s\S]*?<\/\1>/g, '');
}

/** Index just past the string literal that starts at `start`. */
function skipString(text: string, start: number): number {
  const quote = text[start];
  let i = start + 1;
  while (i < text.length && text[i] !== quote) {
    if (text[i] === '\\') i += 1;
    else if (quote === '`' && text[i] === '$' && text[i + 1] === '{') i = skipExpression(text, i + 1) - 1;
    i += 1;
  }
  return i + 1;
}

/** Index just past the `{…}` expression that starts at `start`, scanning any markup inside it. */
function skipExpression(text: string, start: number, found?: string[]): number {
  let depth = 0;
  let i = start;
  let previous = '';
  while (i < text.length) {
    const char = text[i];
    if (char === '"' || char === "'" || char === '`') {
      i = skipString(text, i);
      previous = char;
      continue;
    }
    if (char === '{') depth += 1;
    if (char === '}') {
      depth -= 1;
      if (depth === 0) return i + 1;
    }
    // Markup inside an expression: `(<li>`, `? <a>`, `&& <p>`, `=> <x>`.
    if (char === '<' && /[A-Za-z>]/.test(text[i + 1] ?? '') && /[(?:&|,=>]/.test(previous)) {
      i = scanMarkup(text, i, true, found);
      previous = '>';
      continue;
    }
    if (!/\s/.test(char)) previous = char;
    i += 1;
  }
  return i;
}

/** Index just past the tag that starts at `start`; records literal visible attributes. */
function skipTag(text: string, start: number, found?: string[]): number {
  let i = start + 1;
  while (i < text.length && text[i] !== '>') {
    if (text[i] === '{') {
      i = skipExpression(text, i, found);
      continue;
    }
    const attribute = /^([\w:@.-]+)=(["'])([^"']*)\2/.exec(text.slice(i));
    if (attribute) {
      if (VISIBLE_ATTRIBUTES.has(attribute[1].toLowerCase()) && /\p{L}/u.test(attribute[3])) {
        found?.push(`${attribute[1]}="${attribute[3]}"`);
      }
      i += attribute[0].length;
      continue;
    }
    i += 1;
  }
  return i + 1;
}

/**
 * Scans markup from `start`. Inside an expression it stops after the element
 * that starts there closes; at the top level it runs to the end.
 */
function scanMarkup(text: string, start: number, nested: boolean, found?: string[]): number {
  let depth = 0;
  let i = start;
  let run = '';
  const flush = () => {
    if (/\p{L}/u.test(run)) found?.push(run.trim().replace(/\s+/g, ' '));
    run = '';
  };
  while (i < text.length) {
    const char = text[i];
    if (char === '{') {
      flush();
      i = skipExpression(text, i, found);
      continue;
    }
    if (char === '<') {
      flush();
      const name = /^<\/?([\w.-]*)/.exec(text.slice(i))?.[1] ?? '';
      const closing = text[i + 1] === '/';
      const end = skipTag(text, i, found);
      const selfClosing = text[end - 2] === '/' || text[i + 1] === '!' || VOID_ELEMENTS.has(name.toLowerCase());
      if (closing) depth -= 1;
      else if (!selfClosing) depth += 1;
      i = end;
      if (nested && depth <= 0) return i;
      continue;
    }
    if (nested && depth === 0) return i;
    run += char;
    i += 1;
  }
  flush();
  return i;
}

/** Every piece of literal user-visible text in an .astro file. */
export function literalText(source: string): string[] {
  const found: string[] = [];
  scanMarkup(template(source), 0, false, found);
  return found;
}
