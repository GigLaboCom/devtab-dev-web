import { wordmark } from '../wordmark.ts';

/**
 * English, the source catalogue: every other catalogue has exactly these
 * keys (`satisfies Messages`, checked again when the site builds). Every
 * string a visitor can see lives in a catalogue, never in markup.
 */
const en = {
  /** The language's own name, as the language switch shows it. */
  language: { name: 'English' },
  site: {
    name: wordmark,
    skipToContent: 'Skip to content',
    themeSystem: 'Theme: as the system’s. Switch to light',
    themeLight: 'Theme: light. Switch to dark',
    themeDark: 'Theme: dark. Switch to the system’s',
    languageSwitch: 'Language',
  },
  /** One entry per page of `src/lib/pages.ts`: its title and description. */
  pages: {
    home: {
      title: wordmark,
      description: 'A Chrome extension for comparing and reviewing code.',
      heading: wordmark,
      lede: 'A Chrome extension for comparing and reviewing code.',
      placeholder: 'This site is a placeholder. There is nothing to read here yet.',
    },
    /**
     * The plans page, /pro/. The owner has not given its texts yet: these
     * are the specifications' words where they have any, otherwise the
     * shortest factual ones (README.md, "Texts awaiting the owner"). The
     * plans' own texts come from the API.
     */
    pro: {
      title: 'Pro plans',
      description: 'The Pro plans and their prices.',
      heading: 'Pro plans',
      listLabel: 'Pro plans',
      perMonth: 'per month',
      perYear: 'per year',
      perMemberMonth: 'per member a month',
      perMemberYear: 'per member a year',
      fromMembers: '{price} each from {count} members',
      teamsHeading: 'For teams',
      teamsIntro: 'One subscription for the whole team. Invite members by e-mail; each connects their own browsers.',
      free: 'Free',
      freeNote: 'Free forever',
      choose: 'Choose {name}',
      soon: 'Soon',
      notOpen: 'Purchases are not open yet.',
      empty: 'No plans are offered yet.',
      unavailable: 'Couldn’t load the plans. Try again later.',
    },
    /**
     * The sign-in page, /sign-in/: awaiting the owner, like `pro`. Its
     * general strings serve the other pages of signing in too.
     */
    signIn: {
      title: 'Sign in',
      description: 'Sign in with your e-mail address.',
      heading: 'Sign in',
      intro: 'We’ll send you an e-mail with a sign-in link and a code.',
      emailLabel: 'E-mail address',
      send: 'Send the e-mail',
      sent: 'We sent a sign-in link and a code to {email}.',
      sentNote: 'Open the link in this browser, or enter the code here.',
      codeLabel: 'Code',
      verify: 'Sign in',
      otherAddress: 'Use another address',
      signedIn: 'You’re signed in as {email}.',
      signOut: 'Sign out',
      unavailable: 'Sign-in isn’t available right now.',
      or: 'or',
      passwordLabel: 'Password',
      signInPassword: 'Sign in',
      forgot: 'Forgot your password?',
      noAccount: 'No account yet?',
      signUp: 'Create an account',
      passwordChanged: 'Your new password is saved.',
      errorEmail: 'Enter a valid e-mail address.',
      errorSend: 'Couldn’t send the e-mail. Try again later.',
      errorCode: 'This code is wrong or has expired.',
      errorLink: 'This link didn’t work. Enter the code from the e-mail instead.',
      errorRate: 'Too many attempts. Try again later.',
      errorCredentials: 'The e-mail address or the password is wrong.',
      errorUnconfirmed: 'Confirm your address first: open the link or enter the code from the e-mail we sent.',
      errorFailed: 'Couldn’t sign you in. Try again later.',
      errorPasswordWeak: 'This password is too weak. Choose a longer one.',
      errorPasswordLong: 'This password is too long: use at most 72 characters.',
      errorPasswordMismatch: 'The two passwords don’t match.',
    },
    /** /sign-up/: a new account with a password; awaiting the owner. */
    signUp: {
      title: 'Create an account',
      description: 'Create an account with your e-mail address and a password.',
      heading: 'Create an account',
      intro: 'We’ll send you an e-mail to confirm the address.',
      confirmLabel: 'Repeat the password',
      submit: 'Create the account',
      haveAccount: 'Already have an account?',
      signIn: 'Sign in',
      errorExists: 'There is already an account with this address. Sign in instead.',
    },
    /** /forgot-password/: a mail to set a new password; awaiting the owner. */
    forgotPassword: {
      title: 'Forgot your password',
      description: 'Get an e-mail to set a new password.',
      heading: 'Forgot your password',
      intro: 'We’ll send you an e-mail with a link and a code to set a new password.',
      sent: 'If there is an account with {email}, we sent it a link and a code.',
      verify: 'Continue',
      back: 'Back to sign-in',
      errorLink: 'This link didn’t work. Enter the code from the e-mail, or ask for a new one.',
    },
    /** /reset-password/: the new password, after the mail's link or code; awaiting the owner. */
    resetPassword: {
      title: 'Set a new password',
      description: 'Choose a new password for your account.',
      heading: 'Set a new password',
      intro: 'Choose a new password for {email}.',
      passwordLabel: 'New password',
      confirmLabel: 'Repeat the new password',
      submit: 'Save the password',
      expired: 'This page works only right after you open the link, or enter the code, of a password reset e-mail.',
      again: 'Ask for a new e-mail',
      errorSame: 'The new password must differ from the old one.',
      errorUpdate: 'Couldn’t save the password. Try again later.',
    },
    /** /pro/connect/: connecting the extension to the account; awaiting the owner. */
    connect: {
      title: 'Connect the extension',
      description: 'Connect the extension in this browser to your account.',
      heading: 'Connect the extension',
      working: 'Connecting the extension in this browser…',
      connected: 'Done: the extension in this browser now has your licence.',
      noExtension: 'The extension isn’t installed in this browser, or it’s a version that can’t connect.',
      noLicence: 'Your account has no active licence yet.',
      seats: 'Your licence is active in as many browsers as it allows. Disconnect one first.',
      inactive: 'Your licence isn’t active: the subscription ended or the key was blocked.',
      failed: 'Couldn’t connect the extension. Try again, or enter your licence key in the extension.',
      keyNote: 'You can always enter your licence key in the extension instead.',
      plans: 'See the plans',
    },
  },
  notFound: {
    title: 'Page not found',
    description: 'There is no page at this address.',
    heading: 'Page not found',
    message: 'There is no page at this address.',
    home: 'Go to the home page',
  },
};

export default en;

/** The shape every catalogue has: English's keys, any strings. */
export type Messages = typeof en;
