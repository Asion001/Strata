import type { User } from './api';

export const session = $state<{ user: User | null; checked: boolean }>({ user: null, checked: false });
