// Preloads the full images next to the one on show, and lets go of the rest:
// an image the viewer moved away from is released (its request cancelled,
// its decoded pixels free to go) instead of piling up while someone flicks
// through a big folder.

export class Preloader {
  private held = new Map<string, HTMLImageElement>();

  /** Hold exactly these URLs (starting the ones not held yet). */
  keep(urls: (string | null | undefined)[]) {
    const want = new Set(urls.filter((u): u is string => !!u));
    for (const [url, img] of this.held) {
      if (!want.has(url)) this.drop(url, img);
    }
    for (const url of want) {
      if (this.held.has(url)) continue;
      const img = new Image();
      img.decoding = 'async';
      img.src = url;
      this.held.set(url, img);
    }
  }

  /** What is held now (for tests). */
  get urls() {
    return [...this.held.keys()];
  }

  clear() {
    for (const [url, img] of this.held) this.drop(url, img);
  }

  private drop(url: string, img: HTMLImageElement) {
    // An empty src cancels a load in flight and frees the decoded image.
    img.removeAttribute('src');
    this.held.delete(url);
  }
}
