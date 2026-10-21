package id.or.oo.pr.session

fun interface DistroCommand {
    suspend fun run(args: List<String>, onLine: (String) -> Unit): Int
}
