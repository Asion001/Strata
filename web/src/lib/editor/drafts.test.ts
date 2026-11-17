import 'fake-indexeddb/auto';
import { describe, expect, it } from 'vitest';
import { clearDraft, draftKey, getDraft, saveDraft } from './drafts';

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
});
