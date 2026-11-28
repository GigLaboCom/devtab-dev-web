/**
 * The classes the pages of signing in share: their fields, buttons and links,
 * from the design tokens only (src/styles/tokens.css).
 */
export const ui = {
  label: 'block text-sm font-medium',
  field: 'mt-1.5 w-full rounded-md border border-border bg-background px-3 py-2 text-sm',
  primary:
    'mt-5 flex w-full justify-center rounded-md bg-primary px-4 py-2.5 text-sm font-medium text-primary-foreground hover:opacity-90',
  /** A form's button when another form on the page has the primary one. */
  alternative:
    'mt-5 flex w-full justify-center rounded-md border border-border bg-background px-4 py-2.5 text-sm font-medium hover:bg-muted',
  secondary:
    'flex w-full items-center justify-center gap-2 rounded-md border border-border bg-background px-4 py-2.5 text-sm font-medium hover:bg-muted',
  link: 'font-medium text-accent underline-offset-4 hover:underline',
  quiet: 'text-sm text-muted-foreground underline underline-offset-4 hover:text-foreground',
  note: 'text-sm text-muted-foreground',
} as const;
