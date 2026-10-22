package id.or.oo.pr.session;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertThrows;

import id.or.oo.pr.engine.ProotHost;
import java.io.File;
import java.nio.file.Files;
import java.util.Collections;
import java.util.List;
import kotlin.Unit;
import kotlin.io.FilesKt;
import kotlin.jvm.functions.Function3;
import org.junit.After;
import org.junit.Test;

public class JavaHostTest {

    private final File root = createRoot();
    private final File prCli = new File(new File(root, "usr"), "bin/pr-cli");

    private final ProotHost host = new ProotHost() {
        @Override
        public File getPrefixDir() {
            return new File(root, "usr");
        }

        @Override
        public File getHomeDir() {
            return new File(root, "home");
        }

        @Override
        public String getPackageName() {
            return "org.example.host";
        }

        @Override
        public File getCacheDir() {
            return new File(root, "cache");
        }

        @Override
        public List<String> getDnsServers() {
            return Collections.emptyList();
        }
    };

    private final Function3<byte[], Integer, Integer, Unit> sink = (buf, off, len) -> Unit.INSTANCE;

    private static File createRoot() {
        try {
            return Files.createTempDirectory("java-host").toFile();
        } catch (java.io.IOException e) {
            throw new IllegalStateException(e);
        }
    }

    @After
    public void tearDown() {
        FilesKt.deleteRecursively(root);
    }

    @Test
    public void aJavaHostBuildsATerminalFactoryWithTheHostAlone() {
        ProotSessionFactory factory = new ProotSessionFactory(host);

        IllegalStateException e = assertThrows(
            IllegalStateException.class,
            () -> factory.create("debian", session -> Unit.INSTANCE)
        );

        assertEquals("could not start a terminal for debian: no pr-cli at " + prCli, e.getMessage());
    }

    @Test
    public void aJavaHostBuildsATerminalFactoryWithBinds() throws java.io.IOException {
        prCli.getParentFile().mkdirs();
        prCli.createNewFile();
        File missing = new File(root, "absent");
        ProotSessionFactory factory =
            new ProotSessionFactory(host, Collections.singletonList(new Bind(missing.getPath(), "/mnt")));

        IllegalStateException e = assertThrows(
            IllegalStateException.class,
            () -> factory.create("debian", session -> Unit.INSTANCE)
        );

        assertEquals("could not start a terminal for debian: no bind source " + missing, e.getMessage());
    }

    @Test
    public void aJavaHostBuildsACommandFactoryWithoutBindsOrASignalWait() {
        CommandSessionFactory factory = new CommandSessionFactory(host, "debian", "exec server", sink);

        IllegalStateException e = assertThrows(
            IllegalStateException.class,
            () -> factory.create("web-server", session -> Unit.INSTANCE)
        );

        assertEquals("could not start a command session for web-server: no pr-cli at " + prCli, e.getMessage());
    }

    @Test
    public void aJavaHostBuildsACommandFactoryWithBindsAndASignalWait() {
        List<Bind> binds = Collections.singletonList(new Bind(root.getPath(), "/srv"));
        List<CommandSessionFactory> factories = List.of(
            new CommandSessionFactory(host, "debian", "exec server", sink, binds),
            new CommandSessionFactory(host, "debian", "exec server", sink, binds, 65_000L)
        );

        for (CommandSessionFactory factory : factories) {
            IllegalStateException e = assertThrows(
                IllegalStateException.class,
                () -> factory.create("web-server", session -> Unit.INSTANCE)
            );
            assertEquals("could not start a command session for web-server: no pr-cli at " + prCli, e.getMessage());
        }
    }
}
