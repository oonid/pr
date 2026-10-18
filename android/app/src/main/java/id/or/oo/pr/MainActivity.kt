package id.or.oo.pr

import android.content.Intent
import android.os.Build
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.BugReport
import androidx.compose.material.icons.filled.Delete
import androidx.compose.material.icons.filled.Download
import androidx.compose.material.icons.filled.MoreVert
import androidx.compose.material.icons.filled.PlayArrow
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.core.view.WindowCompat
import id.or.oo.pr.session.ManagedSession
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.launch
import kotlinx.coroutines.withTimeoutOrNull
import kotlinx.coroutines.withContext
import java.io.File

data class DistroInfo(
    val alias: String,
    val displayName: String,
    val installSource: String,
    val isInstalled: Boolean,
)

private data class DistroCatalogEntry(
    val alias: String,
    val displayName: String,
    val installSource: String,
)

private data class OciImageSuggestion(
    val title: String,
    val imageRef: String,
    val alias: String,
)

private val DISTRO_CATALOG = listOf(
    DistroCatalogEntry("alpine", "alpine:latest", "docker.io/library/alpine:latest"),
    DistroCatalogEntry("alpine-edge", "alpine:edge", "docker.io/library/alpine:edge"),
    DistroCatalogEntry("archlinux", "Arch Linux", "docker.io/library/archlinux:latest"),
    DistroCatalogEntry("debian", "debian:stable", "docker.io/library/debian:stable"),
    DistroCatalogEntry("debian-testing", "debian:testing", "docker.io/library/debian:testing"),
    DistroCatalogEntry("fedora", "Fedora", "registry.fedoraproject.org/fedora:latest"),
    DistroCatalogEntry("manjaro", "Manjaro", "docker.io/manjarolinux/base:latest"),
    DistroCatalogEntry("opensuse", "openSUSE", "registry.opensuse.org/opensuse/tumbleweed:latest"),
    DistroCatalogEntry("rockylinux", "Rocky Linux", "docker.io/library/rockylinux:latest"),
    DistroCatalogEntry("ubuntu", "ubuntu:latest", "docker.io/library/ubuntu:latest"),
)

private val OCI_IMAGE_SUGGESTIONS = listOf(
    OciImageSuggestion("Alpine Latest", "docker.io/library/alpine:latest", "alpine"),
    OciImageSuggestion("Alpine Edge", "docker.io/library/alpine:edge", "alpine-edge"),
    OciImageSuggestion("Ubuntu Latest", "docker.io/library/ubuntu:latest", "ubuntu"),
    OciImageSuggestion("Debian Stable", "docker.io/library/debian:stable", "debian"),
    OciImageSuggestion("Debian Testing", "docker.io/library/debian:testing", "debian-testing"),
    OciImageSuggestion("Fedora Latest", "registry.fedoraproject.org/fedora:latest", "fedora"),
    OciImageSuggestion("Arch Linux", "docker.io/library/archlinux:latest", "archlinux"),
)

private fun listDirectories(parent: File): List<String> {
    return parent.listFiles()
        ?.filter { it.isDirectory }
        ?.map { it.name }
        ?: emptyList()
}

class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        // Edge-to-edge: Compose owns all insets via imePadding() / navigationBarsPadding().
        // Manifest uses adjustNothing so the window never auto-resizes for the keyboard.
        WindowCompat.setDecorFitsSystemWindows(window, false)
        val app = application as App
        setContent {
            val darkTheme = isSystemInDarkTheme()
            val colorScheme = when {
                Build.VERSION.SDK_INT >= Build.VERSION_CODES.S -> {
                    val context = this
                    if (darkTheme) dynamicDarkColorScheme(context)
                    else dynamicLightColorScheme(context)
                }
                darkTheme -> darkColorScheme()
                else -> lightColorScheme()
            }
            MaterialTheme(colorScheme = colorScheme) {
                Surface(
                    modifier = Modifier.fillMaxSize(),
                    color = MaterialTheme.colorScheme.background
                ) {
                    DistroListScreen(app)
                }
            }
        }
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun DistroListScreen(app: App) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var distros by remember { mutableStateOf<List<DistroInfo>>(emptyList()) }
    var loadingDistro by remember { mutableStateOf<String?>(null) }
    var outputLines by remember { mutableStateOf(listOf<String>()) }
    var showOutput by remember { mutableStateOf(false) }
    var operationLabel by remember { mutableStateOf("") }
    var operationDistroName by remember { mutableStateOf("") }
    var customImageRef by remember { mutableStateOf("") }
    var customAlias by remember { mutableStateOf("") }
    val sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)
    val running by app.sessions.running.collectAsState()

    fun refreshDistros() {
        val legacyRootfsDir = File(app.prefixDir, "var/lib/pr/installed-rootfs")
        val ociContainersDir = File(app.prefixDir, "var/lib/pr/containers")
        val installedAliases = mutableSetOf<String>()
        installedAliases.addAll(listDirectories(legacyRootfsDir))
        for (alias in listDirectories(ociContainersDir)) {
            if (File(ociContainersDir, "$alias/rootfs").isDirectory) {
                installedAliases.add(alias)
            }
        }

        val catalogDistros = DISTRO_CATALOG.map { entry ->
            DistroInfo(
                alias = entry.alias,
                displayName = entry.displayName,
                installSource = entry.installSource,
                isInstalled = installedAliases.contains(entry.alias)
            )
        }
        val catalogAliases = DISTRO_CATALOG.map { it.alias }.toSet()
        val runtimeOnlyDistros = installedAliases
            .filter { !catalogAliases.contains(it) }
            .map { alias ->
                DistroInfo(
                    alias = alias,
                    displayName = alias,
                    installSource = alias,
                    isInstalled = true
                )
            }

        distros = (catalogDistros + runtimeOnlyDistros).sortedBy { it.displayName.lowercase() }
    }

    LaunchedEffect(Unit) { refreshDistros() }

    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text("PR") },
                colors = TopAppBarDefaults.topAppBarColors(
                    containerColor = MaterialTheme.colorScheme.primaryContainer,
                )
            )
        }
    ) { padding ->
        Column(modifier = Modifier.fillMaxSize().padding(padding)) {
            SessionBanners(app)
            // Distro list — always visible behind the sheet
            LazyColumn(
                modifier = Modifier
                    .fillMaxWidth()
                    .weight(1f)
            ) {
                item {
                    CustomImageInstallCard(
                        imageRef = customImageRef,
                        alias = customAlias,
                        isLoading = loadingDistro != null,
                        onImageRefChange = { customImageRef = it },
                        onAliasChange = { customAlias = it },
                        onInstall = {
                            if (loadingDistro != null) return@CustomImageInstallCard
                            val source = customImageRef.trim()
                            if (source.isEmpty()) return@CustomImageInstallCard
                            val alias = customAlias.trim()
                            if (alias.isNotEmpty() && !isValidOverrideAlias(alias)) {
                                showOutput = true
                                outputLines = listOf(
                                    "ERROR: Invalid alias '$alias'.",
                                    "Alias must start with an alphanumeric character and use only [A-Za-z0-9_.+-]."
                                )
                                return@CustomImageInstallCard
                            }
                            val args = mutableListOf("install", source)
                            if (alias.isNotEmpty()) {
                                args += listOf("--override-alias", alias)
                            }
                            loadingDistro = "__custom__"
                            operationLabel = "Installing"
                            operationDistroName = source
                            showOutput = true
                            outputLines = listOf("Installing $source…")
                            scope.launch {
                                runDistroCommand(app, args) { line ->
                                    outputLines = outputLines + line
                                }
                                loadingDistro = null
                            }
                        }
                    )
                }
                items(distros) { distro ->
                    DistroCard(
                        distro = distro,
                        isLoading = loadingDistro == distro.alias,
                        isRunning = distro.alias in running,
                        onInstall = {
                            if (loadingDistro == null) {
                                loadingDistro = distro.alias
                                operationLabel = "Installing"
                                operationDistroName = distro.displayName
                                showOutput = true
                                outputLines = listOf("Installing ${distro.displayName}…")
                                scope.launch {
                                    runDistroCommand(
                                        app,
                                        listOf(
                                            "install",
                                            distro.installSource,
                                            "--override-alias",
                                            distro.alias
                                        )
                                    ) { line ->
                                        outputLines = outputLines + line
                                    }
                                    loadingDistro = null
                                }
                            }
                        },
                        onLogin = {
                            if (loadingDistro == null) {
                                val intent = Intent(context, TerminalActivity::class.java)
                                intent.putExtra("distro", distro.alias)
                                context.startActivity(intent)
                            }
                        },
                        onRemove = {
                            if (loadingDistro == null) {
                                loadingDistro = distro.alias
                                operationLabel = "Removing"
                                operationDistroName = distro.displayName
                                showOutput = true
                                outputLines = listOf("Removing ${distro.displayName}…")
                                scope.launch {
                                    if (distro.alias in app.sessions.running.value) {
                                        app.sessions.end(distro.alias)
                                        val stopped = withTimeoutOrNull(ManagedSession.END_TIMEOUT_MS) {
                                            app.sessions.running.first { distro.alias !in it }
                                        }
                                        if (stopped == null) {
                                            outputLines = outputLines +
                                                "Couldn't stop the ${distro.displayName} session; removal cancelled."
                                            loadingDistro = null
                                            return@launch
                                        }
                                    }
                                    runDistroCommand(app, listOf("remove", distro.alias)) { line ->
                                        outputLines = outputLines + line
                                    }
                                    loadingDistro = null
                                }
                            }
                        },
                        onTest = {
                            if (loadingDistro == null) {
                                loadingDistro = distro.alias
                                operationLabel = "Testing"
                                operationDistroName = distro.displayName
                                showOutput = true
                                outputLines = listOf("Testing ${distro.displayName}…")
                                scope.launch {
                                    runDistroCommand(app, listOf("test", distro.alias)) { line ->
                                        outputLines = outputLines + line
                                    }
                                    loadingDistro = null
                                }
                            }
                        },
                    )
                }
            }
        }

        // Output console as a bottom sheet overlay.
        // ModalBottomSheet renders as a popup window so it sits above the list
        // and handles IME insets automatically — text never hides behind the keyboard.
        if (showOutput) {
            ModalBottomSheet(
                onDismissRequest = {
                    // Prevent swipe-dismiss while an operation is still running
                    if (loadingDistro == null) {
                        showOutput = false
                        outputLines = emptyList()
                        refreshDistros()
                    }
                },
                sheetState = sheetState,
            ) {
                OutputConsoleContent(
                    operationLabel = operationLabel,
                    distroName = operationDistroName,
                    isRunning = loadingDistro != null,
                    outputLines = outputLines,
                    onClose = {
                        showOutput = false
                        outputLines = emptyList()
                        refreshDistros()
                    }
                )
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Custom OCI install card
// ---------------------------------------------------------------------------

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun CustomImageInstallCard(
    imageRef: String,
    alias: String,
    isLoading: Boolean,
    onImageRefChange: (String) -> Unit,
    onAliasChange: (String) -> Unit,
    onInstall: () -> Unit,
) {
    var showSuggestions by remember { mutableStateOf(false) }

    ElevatedCard(
        modifier = Modifier
            .fillMaxWidth()
            .padding(horizontal = 12.dp, vertical = 6.dp)
    ) {
        Column(
            modifier = Modifier
                .fillMaxWidth()
                .padding(horizontal = 16.dp, vertical = 12.dp)
        ) {
            Row(
                modifier = Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.SpaceBetween,
                verticalAlignment = Alignment.CenterVertically
            ) {
                Text(
                    text = "Custom OCI image",
                    style = MaterialTheme.typography.titleMedium,
                    fontWeight = FontWeight.SemiBold
                )
                TextButton(onClick = { showSuggestions = true }) {
                    Text("ℹ️ Images")
                }
            }
            Spacer(Modifier.height(8.dp))
            OutlinedTextField(
                value = imageRef,
                onValueChange = onImageRefChange,
                label = { Text("Image reference") },
                placeholder = { Text("docker.io/library/debian:stable") },
                singleLine = true,
                modifier = Modifier.fillMaxWidth()
            )
            Spacer(Modifier.height(8.dp))
            OutlinedTextField(
                value = alias,
                onValueChange = onAliasChange,
                label = { Text("Alias (optional)") },
                placeholder = { Text("debian-custom") },
                singleLine = true,
                modifier = Modifier.fillMaxWidth()
            )
            Spacer(Modifier.height(12.dp))
            Button(
                onClick = onInstall,
                enabled = !isLoading && imageRef.trim().isNotEmpty(),
                modifier = Modifier.align(Alignment.End)
            ) {
                Icon(
                    Icons.Default.Download,
                    contentDescription = null,
                    modifier = Modifier.size(ButtonDefaults.IconSize)
                )
                Spacer(Modifier.width(ButtonDefaults.IconSpacing))
                Text("Install")
            }
        }
    }

    if (showSuggestions) {
        AlertDialog(
            onDismissRequest = { showSuggestions = false },
            title = { Text("Choose OCI image") },
            text = {
                Column(
                    modifier = Modifier
                        .fillMaxWidth()
                        .heightIn(max = 320.dp)
                        .verticalScroll(rememberScrollState()),
                    verticalArrangement = Arrangement.spacedBy(8.dp)
                ) {
                    for (item in OCI_IMAGE_SUGGESTIONS) {
                        OutlinedButton(
                            onClick = {
                                onImageRefChange(item.imageRef)
                                onAliasChange(item.alias)
                                showSuggestions = false
                            },
                            modifier = Modifier.fillMaxWidth()
                        ) {
                            Column(
                                modifier = Modifier.fillMaxWidth(),
                                horizontalAlignment = Alignment.Start
                            ) {
                                Text(item.title, fontWeight = FontWeight.Medium)
                                Text(item.imageRef, style = MaterialTheme.typography.bodySmall)
                            }
                        }
                    }
                }
            },
            confirmButton = {
                TextButton(onClick = { showSuggestions = false }) {
                    Text("Close")
                }
            }
        )
    }
}

// ---------------------------------------------------------------------------
// Distro card
// ---------------------------------------------------------------------------

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun DistroCard(
    distro: DistroInfo,
    isLoading: Boolean,
    isRunning: Boolean,
    onInstall: () -> Unit,
    onLogin: () -> Unit,
    onRemove: () -> Unit,
    onTest: () -> Unit,
) {
    var showMenu by remember { mutableStateOf(false) }

    ElevatedCard(
        modifier = Modifier
            .fillMaxWidth()
            .padding(horizontal = 12.dp, vertical = 6.dp)
    ) {
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .padding(horizontal = 16.dp, vertical = 12.dp),
            horizontalArrangement = Arrangement.SpaceBetween,
            verticalAlignment = Alignment.CenterVertically
        ) {
            // Name + status
            Column(modifier = Modifier.weight(1f)) {
                Text(
                    text = distro.displayName,
                    style = MaterialTheme.typography.titleMedium,
                    fontWeight = FontWeight.SemiBold
                )
                Spacer(Modifier.height(4.dp))
                Text(
                    text = when {
                        isRunning -> "● Running"
                        distro.isInstalled -> "● Installed"
                        else -> "○ Not installed"
                    },
                    style = MaterialTheme.typography.labelSmall,
                    color = if (distro.isInstalled)
                        MaterialTheme.colorScheme.primary
                    else
                        MaterialTheme.colorScheme.outline
                )
            }

            // Actions
            if (isLoading) {
                CircularProgressIndicator(
                    modifier = Modifier.size(24.dp),
                    strokeWidth = 2.dp
                )
            } else if (distro.isInstalled) {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    // Primary action: login
                    FilledIconButton(onClick = onLogin) {
                        Icon(Icons.Default.PlayArrow, contentDescription = "Login")
                    }
                    // Secondary actions: test + remove in overflow menu
                    Box {
                        IconButton(onClick = { showMenu = true }) {
                            Icon(Icons.Default.MoreVert, contentDescription = "More options")
                        }
                        DropdownMenu(
                            expanded = showMenu,
                            onDismissRequest = { showMenu = false }
                        ) {
                            DropdownMenuItem(
                                text = { Text("Test") },
                                leadingIcon = {
                                    Icon(Icons.Default.BugReport, contentDescription = null)
                                },
                                onClick = {
                                    showMenu = false
                                    onTest()
                                }
                            )
                            HorizontalDivider()
                            DropdownMenuItem(
                                text = {
                                    Text(
                                        "Remove",
                                        color = MaterialTheme.colorScheme.error
                                    )
                                },
                                leadingIcon = {
                                    Icon(
                                        Icons.Default.Delete,
                                        contentDescription = null,
                                        tint = MaterialTheme.colorScheme.error
                                    )
                                },
                                onClick = {
                                    showMenu = false
                                    onRemove()
                                }
                            )
                        }
                    }
                }
            } else {
                OutlinedButton(
                    onClick = onInstall,
                    contentPadding = PaddingValues(horizontal = 16.dp, vertical = 8.dp)
                ) {
                    Icon(
                        Icons.Default.Download,
                        contentDescription = null,
                        modifier = Modifier.size(ButtonDefaults.IconSize)
                    )
                    Spacer(Modifier.width(ButtonDefaults.IconSpacing))
                    Text("Install")
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Output console (rendered inside ModalBottomSheet)
// ---------------------------------------------------------------------------

@Composable
fun OutputConsoleContent(
    operationLabel: String,
    distroName: String,
    isRunning: Boolean,
    outputLines: List<String>,
    onClose: () -> Unit,
) {
    val scrollState = rememberScrollState()
    // Auto-scroll to latest output as new lines arrive
    LaunchedEffect(outputLines.size) {
        scrollState.animateScrollTo(Int.MAX_VALUE)
    }

    Column(
        modifier = Modifier
            .fillMaxWidth()
            .padding(horizontal = 16.dp)
            .navigationBarsPadding()
    ) {
        // Header: operation label + progress
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .padding(bottom = 12.dp),
            verticalAlignment = Alignment.CenterVertically
        ) {
            if (isRunning) {
                CircularProgressIndicator(
                    modifier = Modifier.size(18.dp),
                    strokeWidth = 2.dp
                )
                Spacer(Modifier.width(10.dp))
            }
            Column(modifier = Modifier.weight(1f)) {
                Text(
                    "$operationLabel: $distroName",
                    style = MaterialTheme.typography.titleSmall,
                    fontWeight = FontWeight.Medium
                )
                if (!isRunning) {
                    Text(
                        "Completed",
                        style = MaterialTheme.typography.labelSmall,
                        color = MaterialTheme.colorScheme.primary
                    )
                }
            }
        }

        // Scrollable output area with monospace text
        Surface(
            modifier = Modifier
                .fillMaxWidth()
                .heightIn(min = 200.dp, max = 400.dp),
            shape = MaterialTheme.shapes.medium,
            color = MaterialTheme.colorScheme.surfaceVariant,
            tonalElevation = 1.dp,
        ) {
            SelectionContainer(
                modifier = Modifier
                    .verticalScroll(scrollState)
                    .padding(12.dp)
            ) {
                Text(
                    text = outputLines.joinToString("\n"),
                    fontFamily = FontFamily.Monospace,
                    style = MaterialTheme.typography.bodySmall,
                )
            }
        }

        Spacer(Modifier.height(12.dp))

        // Close button — disabled while the operation is still running
        Button(
            onClick = onClose,
            enabled = !isRunning,
            modifier = Modifier.fillMaxWidth()
        ) {
            Text(if (isRunning) "Running…" else "Close")
        }

        Spacer(Modifier.height(8.dp))
    }
}

private suspend fun runDistroCommand(
    app: App,
    args: List<String>,
    onLine: (String) -> Unit
) = withContext(Dispatchers.IO) {
    val binDir = File(app.prefixDir, "bin")

    val env = mapOf(
        "APP_PREFIX" to app.prefixDir.absolutePath,
        "APP_HOME" to app.homeDir.absolutePath,
        "APP_PACKAGE" to app.packageName,
        "PATH" to "${binDir.absolutePath}:/system/bin:/system/xbin",
        "PROOT_NO_SECCOMP" to "1",
        "PROOT_TMP_DIR" to app.cacheDir.absolutePath,
        "HOME" to app.homeDir.absolutePath,
        "TERM" to "xterm-256color",
        "TMPDIR" to app.cacheDir.absolutePath,
        "APP_DNS_SERVERS" to app.dnsServers.joinToString(","),
    )

    val prCli = File(binDir, "pr-cli").absolutePath
    val cmd = arrayOf(prCli, *args.toTypedArray())

    android.util.Log.d("PR", "Running: ${cmd.joinToString(" ")}")
    val pb = ProcessBuilder(*cmd)
    pb.environment().putAll(env)
    pb.redirectErrorStream(true)

    try {
        val proc = pb.start()
        val reader = proc.inputStream.bufferedReader()
        var line: String? = reader.readLine()
        while (line != null) {
            val cleaned = line
                .replace(Regex("\\x1B\\[[0-9;]*[mGKHJ]"), "")
                .replace(Regex("\\x1B\\]\\d+;.*?\\x07"), "")
                .replace(Regex("\r"), "")
                .trim()
            if (cleaned.isNotEmpty()) {
                val finalLine = cleaned
                android.util.Log.d("PR", finalLine)
                withContext(Dispatchers.Main) { onLine(finalLine) }
            }
            line = reader.readLine()
        }
        val exitCode = proc.waitFor()
        android.util.Log.d("PR", "Exit code: $exitCode")
        withContext(Dispatchers.Main) {
            if (exitCode != 0) {
                onLine("Exit code: $exitCode")
            } else {
                onLine("Done.")
            }
        }
    } catch (e: Exception) {
        withContext(Dispatchers.Main) { onLine("ERROR: ${e.message}") }
    }
}

private fun isValidOverrideAlias(alias: String): Boolean {
    if (alias.isEmpty() || alias.endsWith(".sh")) {
        return false
    }
    if (alias.contains('/') || alias.contains('\\') || alias.contains("..")) {
        return false
    }
    val first = alias.firstOrNull() ?: return false
    if (!first.isLetterOrDigit()) {
        return false
    }
    return alias.all { it.isLetterOrDigit() || it == '_' || it == '.' || it == '+' || it == '-' }
}
