declare global {
  namespace App {
    interface PageState {
      /** The file viewer is open (a history entry, so Back closes it). */
      viewer?: boolean;
    }
  }
}
export {};
