import { getCurrentWindow } from '@tauri-apps/api/window'
const config = require('../../config.js')

const supportModal = window.Elm.Electron.SupportModal.init()

supportModal.ports.copyEmail.subscribe((isUrgent) => {
  navigator.clipboard.writeText(isUrgent ? config.SUPPORT_URGENT_EMAIL : config.SUPPORT_EMAIL)
})

supportModal.ports.submitForm.subscribe(async (formData) => {
  const res = await window.fetch(config.PRODUCTION_SERVER + '/pleasenospam', {
    method: 'POST',
    body: JSON.stringify(formData),
    headers: { 'Content-Type': 'application/json' }
  })
  if (res.ok) {
    getCurrentWindow().close()
  } else {
    window.alert('Could not send for some reason.\nTry again, or copy the email and send it manually.')
  }
})
