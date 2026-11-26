// @vitest-environment jsdom
import 'fake-indexeddb/auto';
import { beforeEach, describe, expect, it } from 'vitest';
import { RECENT_KEY } from '$lib/components/files/folder-picker';
import { DRAFT_TTL_MS, touchTabDraft, hasTabDraft, clearTabDraft, clearAfterSave, clearAllDrafts, clearDraft, listDrafts, markOfferSeen, offerSeen, saveTabDraft, draftKey, expireDrafts, getDraft, isSignedIn, keepOnlyUser, markSignedIn, markSignedOut, saveDraft } from './drafts';

describe('drafts', () => {
  beforeEach(() => markSignedIn(1));

  it('a touched draft keeps its edit time but outlives the TTL', async () => {
    const f = draftKey(1, 1, 'alive');
    expect(await hasTabDraft(f, 'T')).toBe(false);
    await saveTabDraft(f, 'T', { baseEtag: null, text: 'open tab', savedAt: 0 });
    await saveTabDraft(f, 'U', { baseEtag: null, text: 'closed tab', savedAt: 0 });
    expect(await hasTabDraft(f, 'T')).toBe(true);
    touchTabDraft(f, 'T', DRAFT_TTL_MS);
    await expireDrafts(DRAFT_TTL_MS + 1);
    const left = await listDrafts(f);
    expect(left.map((d) => [d.tab, d.savedAt])).toEqual([['T', 0]]);
    await clearTabDraft(f, 'T');
    expect(await hasTabDraft(f, 'T')).toBe(false);
  });

  it('a declined offer stays declined while its draft exists', async () => {
    const f = draftKey(1, 1, 'declined');
    await saveTabDraft(f, 'T', { baseEtag: null, text: 'x', savedAt: 1 });
    const [d] = await listDrafts(f);
    markOfferSeen(d);
    touchTabDraft(f, 'T', 10 * DRAFT_TTL_MS);
    await expireDrafts(10 * DRAFT_TTL_MS);
    expect(offerSeen(d)).toBe(true);
    await clearTabDraft(f, 'T');
    await expireDrafts(10 * DRAFT_TTL_MS);
    expect(offerSeen(d)).toBe(false); // mark pruned with its draft
  });

  it('forgets recent Move/Copy folders when another account signs in', () => {
    markSignedIn(1);
    localStorage.setItem(RECENT_KEY, '["a"]');
    markSignedIn(1);
    expect(localStorage.getItem(RECENT_KEY)).toBe('["a"]');
    markSignedIn(2);
    expect(localStorage.getItem(RECENT_KEY)).toBeNull();
    markSignedOut();
  });

  it('a signed-out browser never recreates the wiped database', async () => {
    await saveDraft({ key: 'k', baseEtag: null, text: 'x', savedAt: 1 });
    localStorage.setItem(RECENT_KEY, '["Medical/2026"]');
    markSignedOut();
    // Recent Move/Copy folders are private paths too.
    expect(localStorage.getItem(RECENT_KEY)).toBeNull();
    await clearAllDrafts();
    // a late writer from another tab
    await expect(saveDraft({ key: 'k', baseEtag: null, text: 'late', savedAt: 2 })).rejects.toBeTruthy();
    markSignedIn(1);
    expect(await getDraft('k')).toBeUndefined();
  });

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

  it('keeps one draft per tab and clears only obsolete ones after a save', async () => {
    const f = draftKey(5, 1, 'x');
    await saveTabDraft(f, 'A', { baseEtag: null, text: 'from A', savedAt: 1 });
    await saveTabDraft(f, 'B', { baseEtag: null, text: 'from B', savedAt: 2 });
    await saveTabDraft(f, 'C', { baseEtag: null, text: 'saved text', savedAt: 3 });
    await saveTabDraft(draftKey(5, 1, 'xy'), 'A', { baseEtag: null, text: 'other file', savedAt: 4 });
    expect((await listDrafts(f)).map((d) => d.text)).toEqual(['saved text', 'from B', 'from A']);
    await clearAfterSave(f, 'A', 'saved text');
    expect((await listDrafts(f)).map((d) => d.text)).toEqual(['from B']);
    expect((await listDrafts(draftKey(5, 1, 'xy'))).length).toBe(1);
  });

  it('a declined offer is remembered per version without touching the entry', async () => {
    const f = draftKey(6, 1, 'z');
    await saveTabDraft(f, 'A', { baseEtag: null, text: 'live', savedAt: 100 });
    const [d] = await listDrafts(f);
    markOfferSeen(d);
    expect(offerSeen(d)).toBe(true);
    expect((await listDrafts(f))[0].text).toBe('live');
    await saveTabDraft(f, 'A', { baseEtag: null, text: 'live 2', savedAt: 200 });
    expect(offerSeen((await listDrafts(f))[0])).toBe(false);
  });
});
