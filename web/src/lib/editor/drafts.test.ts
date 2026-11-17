import 'fake-indexeddb/auto';
import { describe, expect, it } from 'vitest';
import { DRAFT_TTL_MS, clearAllDrafts, clearDraft, draftKey, expireDrafts, getDraft, saveDraft } from './drafts';

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
});
