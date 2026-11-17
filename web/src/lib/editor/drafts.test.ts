// @vitest-environment jsdom
import 'fake-indexeddb/auto';
import { describe, expect, it } from 'vitest';
import { DRAFT_TTL_MS, clearAllDrafts, clearDraft, clearOwnDraft, draftKey, expireDrafts, getDraft, isSignedIn, keepOnlyUser, markSignedIn, markSignedOut, saveDraft } from './drafts';

describe('drafts', () => {
  it('saves, reads and clears by user, mount and path', async () => {
    const k = draftKey(1, 7, 'docs/a.md');
    expect(k).toBe('1|7|docs/a.md');
    expect(await getDraft(k)).toBeUndefined();
    await saveDraft({ key: k, baseEtag: '"v-1"', text: 'hi', savedAt: 5 });
    expect((await getDraft(k))?.text).toBe('hi');
    expect(await getDraft(draftKey(2, 7, 'docs/a.md'))).toBeUndefined();
    await clearDraft(k);
    expect(await getDraft(k)).toBeUndefined();
  });

  it('expires old drafts and clears everything on sign-out', async () => {
    await saveDraft({ key: 'old', baseEtag: null, text: 'x', savedAt: 0 });
    await saveDraft({ key: 'new', baseEtag: null, text: 'y', savedAt: DRAFT_TTL_MS + 10 });
    await expireDrafts(DRAFT_TTL_MS + 20);
    expect(await getDraft('old')).toBeUndefined();
    expect((await getDraft('new'))?.text).toBe('y');
    await clearAllDrafts();
    expect(await getDraft('new')).toBeUndefined();
  });

  it('keeps only the signed-in user\'s drafts', async () => {
    await saveDraft({ key: draftKey(1, 1, 'a'), baseEtag: null, text: 'mine', savedAt: Date.now() });
    await saveDraft({ key: draftKey(2, 1, 'a'), baseEtag: null, text: 'theirs', savedAt: Date.now() });
    await keepOnlyUser(1);
    expect((await getDraft(draftKey(1, 1, 'a')))?.text).toBe('mine');
    expect(await getDraft(draftKey(2, 1, 'a'))).toBeUndefined();
  });

  it('tracks who is signed in across tabs', () => {
    markSignedIn(3);
    expect(isSignedIn(3)).toBe(true);
    expect(isSignedIn(4)).toBe(false);
    markSignedIn(3); // the same user signing in again keeps it on
    expect(isSignedIn(3)).toBe(true);
    markSignedOut();
    expect(isSignedIn(3)).toBe(false);
  });

  it('a tab only clears drafts it wrote itself', async () => {
    const k = draftKey(5, 1, 'x');
    await saveDraft({ key: k, baseEtag: null, text: 'from B', savedAt: Date.now(), tab: 'B' });
    await clearOwnDraft(k, 'A');
    expect((await getDraft(k))?.text).toBe('from B');
    await clearOwnDraft(k, 'B');
    expect(await getDraft(k)).toBeUndefined();
  });
});
