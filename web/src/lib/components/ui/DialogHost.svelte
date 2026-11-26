<script lang="ts">
  // Shows the dialogs queued by $lib/dialogs, one at a time, in a native
  // <dialog> opened with showModal(): it sits above everything (viewer and
  // editor included), traps focus, makes the page behind inert, and gives
  // focus back to where it was when it closes.
  import { cancelRequest, dialogs, type Request } from '$lib/dialogs.svelte';
  import Button from './button/button.svelte';
  import Input from './input/input.svelte';
  import CopyIcon from '@lucide/svelte/icons/copy';
  import CheckIcon from '@lucide/svelte/icons/check';

  let el = $state<HTMLDialogElement | null>(null);
  let req = $derived<Request | undefined>(dialogs.queue[0]);
  let values = $state<Record<string, string>>({});
  let copied = $state(false);
  // A click only cancels when it also started on the backdrop, not when a
  // text selection inside the dialog ends outside it.
  let downOnBackdrop = false;
  // Focused before the first of a run of dialogs; each reopen would
  // otherwise hand focus back to the previous dialog's button.
  let opener: HTMLElement | null = null;
  // The folder browser loads on first use.
  let Picker = $state<typeof import('$lib/components/files/FolderPicker.svelte').default>();
  $effect(() => {
    if (req?.kind === 'folder' && !Picker)
      import('$lib/components/files/FolderPicker.svelte').then(
        (m) => (Picker = m.default),
        (err) => {
          console.error('could not load the folder picker', err);
          cancelRequest(req);
        }
      );
  });

  // Fresh field values before the new request renders.
  $effect.pre(() => {
    const r = req;
    values = r?.kind === 'form' ? Object.fromEntries(r.opts.fields.map((f) => [f.name, f.value ?? ''])) : {};
    copied = false;
  });

  // Each new request reopens the dialog so autofocus lands in it.
  $effect(() => {
    const d = el;
    const r = req;
    if (!d) return;
    if (d.open) d.close();
    if (!r) {
      // The queue is empty: back to where the user was, if it's still there.
      if (opener?.isConnected) opener.focus();
      opener = null;
      return;
    }
    opener ??= document.activeElement as HTMLElement | null;
    d.showModal();
    // The first field, else the request's default button (data-autofocus).
    const f = d.querySelector<HTMLElement>('[data-autofocus]');
    f?.focus();
    if (f instanceof HTMLInputElement) f.select();
  });

  function cancel() {
    cancelRequest(req);
  }

  function submit(e: SubmitEvent) {
    e.preventDefault();
    const r = req;
    if (!r) return;
    if (r.kind === 'confirm') r.done(true);
    else if (r.kind === 'form') {
      if (!ready) return;
      r.done({ ...values });
    } else if (r.kind === 'alert') r.done();
  }

  let ready = $derived(
    req?.kind !== 'form' || req.opts.fields.every((f) => !f.required || (values[f.name] ?? '').trim() !== '')
  );

  async function copy(text: string) {
    try {
      await navigator.clipboard.writeText(text);
      copied = true;
    } catch {
      // Clipboard blocked: select the text so the user can copy it by hand.
      (el?.querySelector('[data-copy]') as HTMLInputElement | null)?.select();
    }
  }

  const titleId = 'strata-dialog-title';
  const bodyId = 'strata-dialog-body';
</script>

<!-- Escape fires "cancel". Keys stop here, so window shortcuts behind the
     modal (Escape closes the viewer, Ctrl+F, Ctrl+S) don't run. If the
     browser closes the dialog on its own (a second Escape can't be
     cancelled), it reopens for the request still waiting. A click on the
     backdrop lands on the <dialog> itself and cancels. -->
<dialog
  bind:this={el}
  aria-labelledby={titleId}
  aria-describedby={req?.opts.body ? bodyId : undefined}
  oncancel={(e) => { e.preventDefault(); cancel(); }}
  onkeydown={(e) => {
    e.stopPropagation();
    // Habit Ctrl/Cmd+S would open the browser's "Save page as".
    if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 's') e.preventDefault();
  }}
  onclose={() => { if (req && el && !el.open) el.showModal(); }}
  onpointerdown={(e) => (downOnBackdrop = e.target === el)}
  onclick={(e) => { if (e.target === el && downOnBackdrop) cancel(); }}
  class={[
    'm-auto rounded-xl border border-border bg-popover p-0 text-popover-foreground shadow-lg backdrop:bg-black/50',
    // The folder browser: a full-height sheet on phones.
    req?.kind === 'folder'
      ? 'h-[min(36rem,85vh)] w-[calc(100%-2rem)] max-w-lg max-sm:h-dvh max-sm:max-h-none max-sm:w-full max-sm:max-w-none max-sm:rounded-none max-sm:border-0 max-sm:pt-[env(safe-area-inset-top)] max-sm:pb-[env(safe-area-inset-bottom)]'
      : 'w-[calc(100%-2rem)] max-w-md'
  ]}
>
  {#if req?.kind === 'folder'}
    {#key req.id}
      {#if Picker}
        <Picker opts={req.opts} done={req.done} {titleId} />
      {:else}
        <h2 id={titleId} class="p-6 text-lg leading-none font-semibold">{req.opts.title}</h2>
      {/if}
    {/key}
  {:else if req}
    {#key req.id}
      <form class="grid gap-4 p-6" onsubmit={submit}>
        <div class="grid gap-2">
          <h2 id={titleId} class="text-lg leading-none font-semibold">{req.opts.title}</h2>
          {#if req.opts.body}<p id={bodyId} class="text-sm text-muted-foreground">{req.opts.body}</p>{/if}
        </div>

        {#if req.kind === 'form'}
          {#each req.opts.fields as f, i (f.name)}
            <label class="grid gap-1.5 text-sm font-medium">
              {f.label}
              <Input
                type={f.type ?? 'text'}
                value={values[f.name]}
                oninput={(e) => (values[f.name] = (e.currentTarget as HTMLInputElement).value)}
                placeholder={f.placeholder}
                min={f.min}
                autocomplete={f.autocomplete ?? (f.type === 'password' ? 'new-password' : 'off')}
                data-autofocus={i === 0 || undefined}
              />
              {#if f.hint}<span class="text-xs font-normal text-muted-foreground">{f.hint}</span>{/if}
            </label>
          {/each}
        {:else if req.kind === 'alert' && req.opts.copy}
          {@const text = req.opts.copy}
          <div class="flex gap-2">
            <Input data-copy data-autofocus readonly value={text} aria-label="Link" />
            <Button variant="outline" size="icon" aria-label={copied ? 'Copied' : 'Copy'} onclick={() => copy(text)}>
              {#if copied}<CheckIcon />{:else}<CopyIcon />{/if}
            </Button>
          </div>
        {/if}

        <div class="flex flex-col-reverse gap-2 sm:flex-row sm:justify-end">
          {#if req.kind === 'alert'}
            <Button type="submit" data-autofocus={!req.opts.copy || undefined}>{req.opts.ok ?? 'OK'}</Button>
          {:else}
            {@const danger = req.kind === 'confirm' && req.opts.danger}
            <!-- A destructive confirm starts on Cancel, so Enter doesn't delete. -->
            <Button variant="outline" onclick={cancel} data-autofocus={danger || undefined}>{req.opts.cancel ?? 'Cancel'}</Button>
            <Button
              type="submit"
              variant={danger ? 'destructive' : 'default'}
              disabled={!ready}
              data-autofocus={(req.kind === 'confirm' && !danger) || undefined}
            >
              {req.opts.confirm ?? 'OK'}
            </Button>
          {/if}
        </div>
      </form>
    {/key}
  {/if}
</dialog>
