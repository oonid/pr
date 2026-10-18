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
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.core.view.WindowCompat
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
    var distros by remember { mutableStateOf<List<DistroInfo>>(emptyList()) }
    var customImageRef by rememberSaveable { mutableStateOf("") }
    var customAlias by rememberSaveable { mutableStateOf("") }
    val sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)
    val running by app.sessions.running.collectAsState()
    val operation by app.operations.operation.collectAsState()
    val busy = operation?.running == true

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

    fun dismissOutput() {
        if (app.operations.dismiss()) refreshDistros()
    }

    LaunchedEffect(busy) { refreshDistros() }

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
                        isLoading = busy,
                        onImageRefChange = { customImageRef = it },
                        onAliasChange = { customAlias = it },
                        onInstall = { app.operations.installCustom(customImageRef, customAlias) }
                    )
                }
                items(distros) { distro ->
                    DistroCard(
                        distro = distro,
                        isLoading = busy && operation?.custom == false && operation?.alias == distro.alias,
                        isRunning = distro.alias in running,
                        onInstall = {
                            app.operations.install(distro.alias, distro.displayName, distro.installSource)
                        },
                        onLogin = {
                            if (!busy) {
                                val intent = Intent(context, TerminalActivity::class.java)
                                intent.putExtra("distro", distro.alias)
                                context.startActivity(intent)
                            }
                        },
                        onRemove = { app.operations.remove(distro.alias, distro.displayName) },
                        onTest = { app.operations.test(distro.alias, distro.displayName) },
                    )
                }
            }
        }

        // Output console as a bottom sheet overlay.
        // ModalBottomSheet renders as a popup window so it sits above the list
        // and handles IME insets automatically — text never hides behind the keyboard.
        operation?.let { current ->
            ModalBottomSheet(
                onDismissRequest = ::dismissOutput,
                sheetState = sheetState,
            ) {
                OutputConsoleContent(
                    operationLabel = current.kind.label,
                    distroName = current.displayName,
                    isRunning = current.running,
                    outputLines = current.lines,
                    onClose = ::dismissOutput
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
