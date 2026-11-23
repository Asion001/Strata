// Names shared by the app and the service worker.

/** A small cache for who is signed in (read by the service worker). */
export const META = 'strata-meta';
export const USER_KEY = '/__offline/user';
/** On a listing served from the offline cache: when it was saved (ISO). */
export const OFFLINE_AT = 'x-strata-offline-at';
/** Background Sync tag for the upload queue. */
export const SYNC_TAG = 'strata-uploads';
