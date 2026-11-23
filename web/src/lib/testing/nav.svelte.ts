// A stand-in for SvelteKit's shallow routing in component tests: page state
// that is reactive, as `page.state` is in the app.
export const nav = $state({ state: {} as Record<string, unknown>, pushes: 0 });
