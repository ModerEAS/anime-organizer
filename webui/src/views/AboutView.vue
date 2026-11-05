<script setup lang="ts">
import { computed, onMounted, reactive, watch } from 'vue'
import { useRoute } from 'vue-router'
import { api, errorMessage, type AppUpdateState } from '../api'
import { t, valueLabel } from '../i18n'
import { useStatus } from '../stores/status'

const route = useRoute()
const { state, refresh } = useStatus()
const unavailableFeature = computed(() => {
  const value = route.query.unavailable
  return Array.isArray(value) ? value[0] ?? '' : value ?? ''
})
const update = computed(() => state.status?.app_update)
const form = reactive({
  interval: '6h',
  autoApply: true,
  feed: '',
  proxy: '',
  dirty: false,
  saving: false,
  acting: false,
  error: '',
})
const busy = computed(() => ['checking', 'downloading', 'verifying', 'applying'].includes(update.value?.phase ?? ''))
const metadata = reactive({ autoDownload: true, loading: true, saving: false, dirty: false, error: '' })

onMounted(async () => {
  try {
    const settings = await api.metadataSettings()
    metadata.autoDownload = settings.auto_download_alias_db
  } catch (error) {
    metadata.error = errorMessage(error)
  } finally {
    metadata.loading = false
  }
})

async function saveMetadata() {
  metadata.saving = true
  metadata.error = ''
  try {
    const settings = await api.saveMetadataSettings({ auto_download_alias_db: metadata.autoDownload })
    metadata.autoDownload = settings.auto_download_alias_db
    metadata.dirty = false
  } catch (error) {
    metadata.error = errorMessage(error)
  } finally {
    metadata.saving = false
  }
}

watch(update, (value) => {
  if (!value || form.dirty) return
  form.interval = value.interval || '6h'
  form.autoApply = value.auto_apply
  form.feed = value.feed || ''
  form.proxy = value.proxy || ''
}, { immediate: true })

function versionText(value: AppUpdateState, latest: boolean) {
  const version = latest ? value.latest_version : value.current_version
  const commit = latest ? value.latest_commit : value.current_commit
  if (!version) return t('Not checked')
  return commit ? `${version} (${commit.slice(0, 7)})` : version
}

function nextCheck(value: AppUpdateState) {
  if (!value.next_check_unix_ms) return t('Not checked')
  return new Date(value.next_check_unix_ms).toLocaleString()
}

function phaseText(value: AppUpdateState) {
  if (value.update_available && (value.phase === 'up_to_date' || value.phase === 'idle')) {
    return t('Update available')
  }
  return valueLabel(value.phase)
}

async function save() {
  form.saving = true
  form.error = ''
  try {
    await api.saveAppUpdateSettings({
      app_update_interval: form.interval.trim(),
      auto_apply_app_update: form.autoApply,
      app_update_feed: form.feed.trim() || null,
      proxy: form.proxy.trim() || null,
    })
    form.dirty = false
    await refresh()
  } catch (error) {
    form.error = errorMessage(error)
  } finally {
    form.saving = false
  }
}

async function check() {
  form.acting = true
  form.error = ''
  try {
    await api.checkAppUpdate()
    await refresh()
  } catch (error) {
    form.error = errorMessage(error)
  } finally {
    form.acting = false
  }
}

async function apply() {
  if (!window.confirm(t('Update and restart this program?'))) return
  form.acting = true
  form.error = ''
  try {
    await api.applyAppUpdate()
    await refresh()
  } catch (error) {
    form.error = errorMessage(error)
  } finally {
    form.acting = false
  }
}
</script>

<template>
  <p v-if="route.query.unavailable !== undefined" class="alert error" role="alert">{{ t('This page is unavailable in the current build: {feature}', { feature: unavailableFeature }) }}</p>
  <div class="page-header"><div><p class="eyebrow">{{ t('System') }}</p><h1>{{ t('About') }}</h1><p class="page-subtitle">{{ t('Local daemon runtime and compiled feature set.') }}</p></div></div>
  <section class="section-block about-grid"><div><span class="label">{{ t('Daemon version') }}</span><strong>{{ state.health?.version ?? t('Unavailable') }}</strong></div><div><span class="label">{{ t('Uptime') }}</span><strong>{{ state.status ? t('{seconds}s', { seconds: state.status.uptime_seconds }) : t('Unavailable') }}</strong></div><div><span class="label">{{ t('Database') }}</span><code>{{ state.status?.database_path ?? t('Unavailable') }}</code></div></section>
  <section v-if="update" class="section-block" aria-labelledby="updates-heading">
    <div class="section-heading"><div><p class="eyebrow">{{ t('Updates') }}</p><h2 id="updates-heading">{{ t('Updates') }}</h2></div></div>
    <p class="page-subtitle">{{ t('Check GitHub releases and replace this program.') }}</p>
    <div class="about-grid">
      <div><span class="label">{{ t('Current build') }}</span><strong>{{ versionText(update, false) }}</strong><small v-if="update.cargo_target">{{ t('Development build') }}</small></div>
      <div><span class="label">{{ t('Latest release') }}</span><strong>{{ versionText(update, true) }}</strong></div>
      <div><span class="label">{{ t('Update status') }}</span><strong>{{ phaseText(update) }}</strong><small v-if="update.phase === 'downloading'">{{ Math.round(update.percent) }}%</small></div>
      <div><span class="label">{{ t('Network') }}</span><strong>{{ update.proxy_display }}</strong></div>
      <div><span class="label">{{ t('Next check') }}</span><strong>{{ nextCheck(update) }}</strong></div>
    </div>
    <p class="update-message">{{ update.message }}</p>
    <p v-if="update.error" class="alert error" role="alert">{{ update.error }}</p>
    <p v-if="form.error" class="alert error" role="alert">{{ form.error }}</p>
    <form class="form-grid" @submit.prevent="save">
      <label class="form-field"><span>{{ t('Check interval') }}</span><input v-model="form.interval" :placeholder="t('6h, or off to disable')" @input="form.dirty = true"></label>
      <label class="form-field"><span>{{ t('Update feed') }}</span><input v-model="form.feed" :placeholder="t('Empty uses the GitHub latest release')" spellcheck="false" @input="form.dirty = true"></label>
      <label class="form-field"><span>{{ t('Proxy') }}</span><input v-model="form.proxy" :placeholder="t('Empty follows the environment, off forces a direct connection')" spellcheck="false" @input="form.dirty = true"></label>
      <label class="checkbox-field"><input v-model="form.autoApply" type="checkbox" @change="form.dirty = true"><span>{{ t('Automatic replacement') }}</span></label>
      <p class="notice">{{ t('Replace and restart when a newer build is found. Development builds under target/ only show a notice.') }}</p>
      <div class="about-actions">
        <button class="button secondary" type="submit" :disabled="form.saving">{{ t('Save update settings') }}</button>
        <button class="button secondary" type="button" :disabled="busy || form.acting" @click="check">{{ t('Check for updates') }}</button>
        <button class="button primary" type="button" :disabled="busy || form.acting" @click="apply">{{ t('Update and restart') }}</button>
      </div>
    </form>
  </section>
  <section class="section-block" aria-labelledby="metadata-heading">
    <div class="section-heading"><div><p class="eyebrow">{{ t('Metadata') }}</p><h2 id="metadata-heading">{{ t('Metadata data source') }}</h2></div></div>
    <p class="page-subtitle">{{ t('Choose which offline Bangumi data this machine downloads.') }}</p>
    <p v-if="metadata.error" class="alert error" role="alert">{{ metadata.error }}</p>
    <form class="form-grid" @submit.prevent="saveMetadata">
      <label class="checkbox-field"><input v-model="metadata.autoDownload" type="checkbox" :disabled="metadata.loading" @change="metadata.dirty = true"><span>{{ t('Automatically download the weekly full alias database (bangumi.db, about 120 MB)') }}</span></label>
      <p class="notice">{{ t('When disabled, only a bangumi.db already on disk is used; otherwise the program falls back to the small AnimeAtlas database.') }}</p>
      <div class="about-actions"><button class="button secondary" type="submit" :disabled="metadata.saving || metadata.loading || !metadata.dirty">{{ t('Save metadata settings') }}</button></div>
    </form>
  </section>
  <section class="section-block" aria-labelledby="capabilities-heading"><div class="section-heading"><div><p class="eyebrow">{{ t('Runtime') }}</p><h2 id="capabilities-heading">{{ t('Capabilities') }}</h2></div></div><div v-if="state.capabilities" class="capability-list"><span v-for="feature in state.capabilities.features" :key="feature" class="capability">{{ valueLabel(feature) }}</span></div><p v-else class="empty-state">{{ t('Capabilities are unavailable while the daemon is offline.') }}</p></section>
</template>
