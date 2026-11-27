/** Small readers of built HTML, shared by the tests of the static and the server's pages. */

export const decode = (value: string) =>
  value
    .replace(/&quot;/g, '"')
    .replace(/&#39;/g, "'")
    .replace(/&lt;/g, '<')
    .replace(/&gt;/g, '>')
    .replace(/&amp;/g, '&');

/** Every `<name …>` tag of a page, as a map of its attributes. */
export function tags(html: string, name: string): Map<string, string>[] {
  return [...html.matchAll(new RegExp(`<${name}\\b([^>]*)>`, 'gi'))].map(
    ([, attributes]) =>
      new Map([...attributes.matchAll(/([\w:-]+)(?:="([^"]*)")?/g)].map(([, key, value = '']) => [key, decode(value)])),
  );
}

/** A page's text without its markup, whitespace collapsed. */
export const text = (html: string) =>
  decode(
    html
      .replace(/<(script|style)\b[\s\S]*?<\/\1>/gi, ' ')
      .replace(/<[^>]+>/g, ' ')
      .replace(/\s+/g, ' ')
      .trim(),
  );
