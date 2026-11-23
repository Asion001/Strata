// @vitest-environment jsdom
import { beforeEach, describe, expect, it } from 'vitest';
import { forget, remember, resumeAt } from './positions';

describe('media positions', () => {
  beforeEach(() => localStorage.clear());

  it('resumes where playback stopped', () => {
    expect(resumeAt('a.mp4', 100)).toBe(0);
    remember('a.mp4', 42.5);
    expect(resumeAt('a.mp4', 100)).toBe(42.5);
    expect(resumeAt('b.mp4', 100)).toBe(0);
  });

  it('ignores the first and last seconds and impossible values', () => {
    remember('a.mp4', 3);
    expect(resumeAt('a.mp4', 100)).toBe(0);
    remember('a.mp4', 97);
    expect(resumeAt('a.mp4', 100)).toBe(0);
    remember('a.mp4', 50);
    expect(resumeAt('a.mp4', 20)).toBe(0); // the file got shorter
    expect(resumeAt('a.mp4', NaN)).toBe(0);
    remember('a.mp4', NaN);
    expect(resumeAt('a.mp4', 100)).toBe(50);
  });

  it('forgets a finished file and keeps only recent entries', () => {
    remember('a.mp4', 50);
    forget('a.mp4');
    expect(resumeAt('a.mp4', 100)).toBe(0);
    for (let i = 0; i < 205; i++) remember(`f${i}`, 50, i);
    expect(resumeAt('f0', 100)).toBe(0);
    expect(resumeAt('f204', 100)).toBe(50);
    expect(Object.keys(JSON.parse(localStorage.getItem('strata-media-positions')!)).length).toBe(200);
  });

  it('drops malformed entries and stores', () => {
    localStorage.setItem('strata-media-positions', '[]');
    remember('a.mp4', 50);
    expect(resumeAt('a.mp4', 100)).toBe(50);
    localStorage.setItem('strata-media-positions', JSON.stringify({ x: { t: 5 }, y: 'z', 'a.mp4': { t: 50, at: 1 } }));
    expect(resumeAt('a.mp4', 100)).toBe(50);
    expect(resumeAt('x', 100)).toBe(0);
  });

  it('survives a broken store', () => {
    localStorage.setItem('strata-media-positions', '{not json');
    expect(resumeAt('a.mp4', 100)).toBe(0);
    remember('a.mp4', 50);
    expect(resumeAt('a.mp4', 100)).toBe(50);
  });
});
