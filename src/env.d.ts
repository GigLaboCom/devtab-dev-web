/// <reference types="astro/client" />

declare namespace App {
  interface Locals {
    /**
     * The signed-in person on the pages that read the session (see
     * `src/middleware.ts`), with a token Supabase's keys verified; null
     * otherwise.
     */
    auth: {
      userId: string;
      email: string | null;
      /** The session's id (`session_id`): what a password reset is bound to. */
      sessionId: string | null;
      /** Passed to the API as `Authorization: Bearer …`; never shown, logged or put in a page. */
      accessToken: string;
    } | null;
    /** This request's server client of Supabase Auth, on those pages when sign-in is configured. */
    supabase: import('@supabase/supabase-js').SupabaseClient | null;
  }
}
