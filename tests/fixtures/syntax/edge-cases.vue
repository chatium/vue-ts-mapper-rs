<script setup lang="ts">
import Comp from './Comp.vue'
const settings = { stretch: 1 as number | string }
const ok = true
</script>

<template>
  <!-- each of these once generated invalid TypeScript, which stops type checking of the whole program -->
  <form @submit.prevent="">
    <Comp v-model="settings.stretch as number" />
  </form>
  <div v-if="false">{{ ok }}</div>
  <template v-else">x</template>
  <div @click="/* todo */" />
</template>
