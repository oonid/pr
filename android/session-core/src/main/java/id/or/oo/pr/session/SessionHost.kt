package id.or.oo.pr.session

import android.content.Context
import android.content.Intent

interface SessionHost {
    val sessions: SessionManager
    val notificationIcon: Int
    fun openAppIntent(): Intent
}

internal val Context.sessionHost: SessionHost
    get() = applicationContext as SessionHost
