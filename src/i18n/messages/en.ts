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
      trial: 'Try Pro free for {days} days. No card needed.',
      trialStart: 'Start the free trial',
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
      google: 'Continue with Google',
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
      errorGoogle: 'Couldn’t sign you in with Google. Try again, or use another way.',
      errorPasswordWeak: 'This password is too weak. Choose a longer one.',
      errorPasswordLong: 'This password is too long: use at most 72 characters.',
      errorPasswordMismatch: 'The two passwords don’t match.',
      errorInvite: 'This link has expired or was used already. Sign in with the address you paid with instead.',
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
    /** /pro/checkout/: buying a plan, with or without an account; awaiting the owner. */
    checkout: {
      title: 'Checkout',
      description: 'Buy a plan, with or without an account.',
      heading: 'Checkout',
      membersLabel: 'Members',
      membersNote: 'At least {count}. You can change the number on the payment page, and later.',
      signedInAs: 'You’re buying as {email}: the licence goes to this account.',
      pay: 'Continue to payment',
      guestNote:
        'No account needed: the payment page asks for your e-mail address, and the licence goes to the account of that address. If there is none yet, we’ll e-mail you a link to open it.',
      payGuest: 'Buy without an account',
      signInFirst: 'Sign in to buy: the licence goes to your account.',
      signInToBuy: 'Sign in to buy',
      continueHeading: 'On to the payment',
      continueNote: 'Taking you to the payment page…',
      continue: 'Continue to payment',
      notForSale: 'This plan can’t be bought now.',
      plans: 'See the plans',
      errorUnavailable: 'This plan can’t be bought now.',
      errorMembers: 'Choose from {min} to 1000 members.',
      errorSignedOut: 'You’re not signed in any more. Sign in again to buy.',
      errorGuestOff: 'Buying without an account isn’t available right now. Sign in to buy.',
      errorPayments: 'Couldn’t start the payment. Try again later.',
    },
    /** /pro/trial/: the trial without a card; awaiting the owner. */
    trial: {
      title: 'Free trial',
      description: 'Try Pro free, without a card.',
      heading: 'Free trial',
      intro: 'Try Pro free for {days} days. No card needed, and nothing is charged when it ends.',
      signedInAs: 'You’re starting it as {email}. One trial per e-mail address.',
      start: 'Start the free trial',
      signInNote: 'Sign in to start it: the trial belongs to your account, one per e-mail address.',
      signIn: 'Sign in to start',
      off: 'No free trial is offered right now.',
      plans: 'See the plans',
      startedHeading: 'Your trial has started',
      startedNote: 'It runs until {date}.',
      keyLabel: 'Your licence key',
      keyNote: 'Copy it now: it is shown only once. Connecting from your account needs no key.',
      connect: 'Connect the extension',
      errorUsed: 'This e-mail address has had a free trial already.',
      errorNotEligible: 'The free trial is for those who haven’t bought a licence yet.',
      errorSignedOut: 'You’re not signed in any more. Sign in again to start the trial.',
      errorUnavailable: 'Couldn’t start the trial. Try again later.',
    },
    /** /pro/thanks/: the return from the payment page, and the invite's landing; awaiting the owner. */
    thanks: {
      title: 'Your purchase',
      description: 'How your purchase went.',
      heading: 'Your purchase',
      nothing:
        'There’s nothing to show here. Open this page in the browser you paid in, or sign in with the address you paid with.',
      open: 'Your payment is being confirmed. This takes a few seconds.',
      openGuest:
        'When it’s through, the licence goes to the account of the address you paid with. If there is none yet, we’ll e-mail you a link to open it.',
      reload: 'Reload the page',
      expired: 'This checkout expired before it was paid.',
      done: 'Thank you! Your licence is active.',
      keyLabel: 'Your licence key',
      keyNote: 'Copy it now: it is shown only once. You can enter it in the extension instead of connecting.',
      keyHint: 'Your licence key ends in {hint}.',
      connect: 'Connect the extension',
      connectNote: 'Connect the extension in this browser to your account; then it needs no key.',
      setPassword: 'Set a password',
      passwordNote: 'Optional: to sign in with a password next time. You can always sign in by e-mail.',
      invited:
        'Check your mail: we sent the address you paid with a link to open your account. The licence is there, and so is connecting the extension.',
      existing: 'The licence is in your account. Sign in with the address you paid with to connect the extension.',
      signIn: 'Sign in',
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
