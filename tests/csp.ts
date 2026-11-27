/**
 * The content security policy of a page, shared by the tests of the static
 * pages (site.test.ts) and of the pages the server renders (server.test.ts).
 * Astro writes the policy into a `<meta http-equiv>` tag with the hashes of
 * its own scripts and styles; a page passes when the policy has the site's
 * directives, names no other host, and allows every inline script and style
 * the page carries, which an inline `style` attribute never is.
 */
import { createHash } from 'node:crypto';

/** The directives every page's policy has, as astro.config.mjs states them. */
export const requiredDirectives = ["default-src 'self'", "form-action 'self'", "object-src 'none'", "base-uri 'self'"];

/** Every directive of a policy, by name, with its sources. */
export function directives(policy: string): Map<string, string[]> {
  return new Map(
    policy
      .split(';')
      .map(part => part.trim().split(/\s+/))
      .filter(([name]) => name)
      .map(([name, ...sources]) => [name.toLowerCase(), sources]),
  );
}

/** The policy a page's markup carries, if any. */
export function metaPolicy(html: string): string | undefined {
  const tag = [...html.matchAll(/<meta\b([^>]*)>/gi)]
    .map(([, attributes]) => attributes)
    .find(attributes => /http-equiv="content-security-policy"/i.test(attributes));
  return tag
    ?.match(/content="([^"]*)"/i)?.[1]
    .replace(/&#39;/g, "'")
    .replace(/&quot;/g, '"');
}

const sha256 = (text: string) => `'sha256-${createHash('sha256').update(text).digest('base64')}'`;

/** What is wrong with a page's policy, one line per problem; `policy` defaults to the page's own. */
export function cspProblems(html: string, policy = metaPolicy(html)): string[] {
  if (!policy) return ['no content security policy'];
  const problems: string[] = [];
  const found = directives(policy);
  for (const required of requiredDirectives) {
    const [name, ...sources] = required.split(' ');
    if (found.get(name)?.join(' ') !== sources.join(' ')) problems.push(`not ${required}: ${policy}`);
  }
  for (const [name, sources] of found) {
    for (const source of sources) {
      if (!/^'(self|none|sha256-[A-Za-z0-9+/=]+)'$/.test(source)) problems.push(`${name} allows ${source}`);
    }
  }
  if (/<[a-z][^>]*\sstyle=/i.test(html)) problems.push('an inline style attribute, which the policy blocks');
  for (const [element, directive] of [
    ['script', 'script-src'],
    ['style', 'style-src'],
  ]) {
    for (const [, attributes, body] of html.matchAll(
      new RegExp(`<${element}\\b([^>]*)>([\\s\\S]*?)</${element}>`, 'gi'),
    )) {
      if (/\bsrc=/.test(attributes) || /type="application\/(ld\+)?json"/.test(attributes)) continue;
      if (!(found.get(directive) ?? []).includes(sha256(body))) {
        problems.push(`an inline <${element}> whose hash ${directive} does not allow`);
      }
    }
  }
  return problems;
}
