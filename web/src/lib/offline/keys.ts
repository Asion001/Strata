// Names shared by the app and the service worker.

/** A small cache for who is signed in (read by the service worker). */
export const META = 'strata-meta';
export const USER_KEY = '/__offline/user';
/** In localStorage: the user signed in here last (pages read this, not
 *  `META`: see `signedInHere`). */
export const LAST_USER = 'strata-last-user';
/** On a listing served from the offline cache: when it was saved (ISO). */
export const OFFLINE_AT = 'x-strata-offline-at';
/** Background Sync tag for the upload queue. */
export const SYNC_TAG = 'strata-uploads';
/** On a request: the network's answer or none, never an offline copy. */
export const NETWORK_ONLY = 'x-strata-network-only';
