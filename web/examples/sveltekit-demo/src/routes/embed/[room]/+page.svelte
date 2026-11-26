<script lang="ts">
  import { page } from '$app/state';
  import { StreamChat } from '@linker-chat/svelte';

  const room = $derived(page.params.room || 'main');
  const serverUrl = $derived(page.url.searchParams.get('server') || 'ws://localhost:8080/ws');
  const theme = $derived(page.url.searchParams.get('theme') === 'light' ? 'light' : 'dark');
  const tokenParam = $derived(page.url.searchParams.get('token') || '');
</script>

<div class="embed" data-theme={theme}>
  <StreamChat
    {serverUrl}
    {room}
    {theme}
    embedded
    getToken={() => (tokenParam ? tokenParam : null)}
    allowGuests={true}
  />
</div>

<style>
  .embed {
    position: fixed;
    inset: 0;
    background: #09090b;
  }
  .embed[data-theme='light'] {
    background: #fafafa;
  }
</style>
