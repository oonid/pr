package id.or.oo.pr.session

fun interface CommandRunner {
    suspend fun run(cmd: List<String>, env: Map<String, String>, onLine: (String) -> Unit): Int
}
