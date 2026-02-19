const { invoke } = window.__TAURI__.core;

let isListening = false;

const toggleBtn = document.getElementById("toggle-btn");
const toggleLabel = document.getElementById("toggle-label");
const statusBadge = document.getElementById("status-badge");
const transcriptionText = document.getElementById("transcription-text");
const modelInfo = document.getElementById("model-info");

async function updateStatus() {
  try {
    const status = await invoke("get_status");
    if (status.model) {
      modelInfo.textContent = `Model: ${status.model}`;
    }
  } catch (err) {
    console.error("Failed to get status:", err);
  }
}

toggleBtn.addEventListener("click", async () => {
  if (isListening) {
    try {
      await invoke("stop_listening");
      isListening = false;
      toggleBtn.classList.remove("active");
      toggleLabel.textContent = "Start Listening";
      statusBadge.textContent = "Idle";
      statusBadge.className = "badge idle";
    } catch (err) {
      console.error("Failed to stop listening:", err);
    }
  } else {
    try {
      await invoke("start_listening");
      isListening = true;
      toggleBtn.classList.add("active");
      toggleLabel.textContent = "Stop Listening";
      statusBadge.textContent = "Listening";
      statusBadge.className = "badge listening";
    } catch (err) {
      console.error("Failed to start listening:", err);
    }
  }
});

// Initialize
updateStatus();
